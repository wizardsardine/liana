//! Spark Settings panel.
//!
//! Two cards:
//! - A "Stable Balance" toggle (Phase 6) — enables or disables the
//!   Spark SDK's USD-pegged balance feature via
//!   `update_user_settings(stable_balance_active_label)`.
//! - A "Bridge status" diagnostic card that shows whether the last
//!   `get_info` round-trip to the `coincube-spark-bridge` subprocess
//!   succeeded.
//!
//! Everything else the old Phase 4b panel surfaced — balance,
//! identity pubkey, network display, Default Lightning backend
//! picker — moved elsewhere:
//! - Balance is already rendered in Spark → Overview / Send.
//! - Network lives in **Settings → General**.
//! - Default Lightning backend lives in **Settings → Lightning**.
//! - Identity pubkey was dropped entirely (not actionable for
//!   end users).

use std::sync::Arc;

use coincube_spark_protocol::GetUserSettingsOk;
use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view::spark::{SparkSettingsStatus, SparkSettingsView};
use crate::app::wallets::SparkBackend;

pub struct SparkSettings {
    backend: Option<Arc<SparkBackend>>,
    /// Coarse bridge-reachability state. Reflects whatever the most
    /// recent `get_info` call returned.
    status: SparkSettingsStatus,
    /// Phase 6: latest Stable Balance state read from the bridge.
    /// `None` until the first `get_user_settings` round-trip
    /// completes; the view renders the toggle in a "loading"
    /// state while it's `None`. Updated optimistically when the
    /// user flips the toggle, then reconciled on
    /// `StableBalanceSaved`.
    stable_balance_active: Option<bool>,
    /// Phase 6: `true` while a `set_stable_balance` RPC is in
    /// flight — the toggle is disabled in that window.
    stable_balance_saving: bool,
}

impl SparkSettings {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        let status = if backend.is_some() {
            SparkSettingsStatus::Loading
        } else {
            SparkSettingsStatus::Unavailable
        };
        Self {
            backend,
            status,
            stable_balance_active: None,
            stable_balance_saving: false,
        }
    }
}

impl State for SparkSettings {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        crate::app::view::dashboard(
            menu,
            cache,
            SparkSettingsView {
                status: self.status.clone(),
                stable_balance_active: self.stable_balance_active,
                stable_balance_saving: self.stable_balance_saving,
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
            self.status = SparkSettingsStatus::Unavailable;
            return Task::none();
        };
        self.status = SparkSettingsStatus::Loading;
        // `get_info` is the liveness probe: a success means the bridge
        // subprocess is up and the SDK is past init. `get_user_settings`
        // is fetched in parallel so the Stable Balance toggle reflects
        // the real SDK state.
        let info_task = Task::perform(
            {
                let backend = backend.clone();
                async move { backend.get_info().await }
            },
            |result| match result {
                Ok(_) => Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::BridgeReachable,
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::BridgeError(e.to_string()),
                )),
            },
        );
        let settings_task = Task::perform(
            async move { backend.get_user_settings().await },
            |result: Result<GetUserSettingsOk, _>| match result {
                Ok(settings) => Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::UserSettingsLoaded(settings),
                )),
                Err(e) => {
                    tracing::warn!("get_user_settings failed: {}", e);
                    Message::View(crate::app::view::Message::SparkSettings(
                        crate::app::view::SparkSettingsMessage::UserSettingsLoaded(
                            GetUserSettingsOk {
                                stable_balance_active: false,
                                private_mode_enabled: false,
                            },
                        ),
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
        if let Message::View(crate::app::view::Message::SparkSettings(msg)) = message {
            match msg {
                crate::app::view::SparkSettingsMessage::BridgeReachable => {
                    self.status = SparkSettingsStatus::Connected;
                }
                crate::app::view::SparkSettingsMessage::BridgeError(err) => {
                    self.status = SparkSettingsStatus::Error(err);
                }
                crate::app::view::SparkSettingsMessage::UserSettingsLoaded(settings) => {
                    self.stable_balance_active = Some(settings.stable_balance_active);
                }
                crate::app::view::SparkSettingsMessage::StableBalanceToggled(enabled) => {
                    let Some(backend) = self.backend.clone() else {
                        return Task::none();
                    };
                    // Optimistic update — snap the toggle immediately so
                    // the UI feels responsive. StableBalanceSaved
                    // reconciles if the RPC fails.
                    self.stable_balance_active = Some(enabled);
                    self.stable_balance_saving = true;
                    return Task::perform(
                        async move { backend.set_stable_balance(enabled).await },
                        move |result| match result {
                            Ok(()) => Message::View(crate::app::view::Message::SparkSettings(
                                crate::app::view::SparkSettingsMessage::StableBalanceSaved(Ok(
                                    enabled,
                                )),
                            )),
                            Err(e) => Message::View(crate::app::view::Message::SparkSettings(
                                crate::app::view::SparkSettingsMessage::StableBalanceSaved(Err(
                                    e.to_string()
                                )),
                            )),
                        },
                    );
                }
                crate::app::view::SparkSettingsMessage::StableBalanceSaved(result) => {
                    self.stable_balance_saving = false;
                    match result {
                        Ok(enabled) => {
                            self.stable_balance_active = Some(enabled);
                        }
                        Err(err) => {
                            tracing::warn!("set_stable_balance failed: {}", err);
                            if let Some(current) = self.stable_balance_active {
                                self.stable_balance_active = Some(!current);
                            }
                            self.status = SparkSettingsStatus::Error(err);
                        }
                    }
                }
            }
        }
        Task::none()
    }
}
