//! Spark Settings panel.
//!
//! Renders:
//! - Read-only diagnostics from the bridge's `get_info` RPC
//!   (balance, identity pubkey, network).
//! - A "Default Lightning backend" picker (Phase 5) — selects the
//!   backend that fulfills incoming `@coincube.io` Lightning
//!   Address invoices. Persists to the cube's settings file and
//!   triggers `Message::SettingsSaved` so the rest of the app
//!   picks up the new value.
//! - A "Stable Balance" toggle (Phase 6) — enables or disables the
//!   Spark SDK's USD-pegged balance feature via
//!   `update_user_settings(stable_balance_active_label)`.
//!
//! Signer rotation and a "bridge health / reconnect" control remain
//! future work.

use std::sync::Arc;

use coincube_ui::widget::Element;
use coincube_spark_protocol::{GetInfoOk, GetUserSettingsOk};
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view::spark::SparkSettingsView;
use crate::app::wallets::SparkBackend;

/// What [`SparkSettings::reload`] produces when the bridge answers.
#[derive(Debug, Clone)]
pub struct SparkSettingsSnapshot {
    pub balance_sats: u64,
    pub identity_pubkey: String,
}

/// Phase 4b Spark Settings panel.
pub struct SparkSettings {
    backend: Option<Arc<SparkBackend>>,
    snapshot: Option<SparkSettingsSnapshot>,
    error: Option<String>,
    loading: bool,
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
        Self {
            backend,
            snapshot: None,
            error: None,
            loading: false,
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
        let status = if self.backend.is_none() {
            crate::app::view::spark::SparkSettingsStatus::Unavailable
        } else if self.loading && self.snapshot.is_none() {
            crate::app::view::spark::SparkSettingsStatus::Loading
        } else if let Some(snapshot) = &self.snapshot {
            crate::app::view::spark::SparkSettingsStatus::Loaded(snapshot.clone())
        } else if let Some(err) = &self.error {
            crate::app::view::spark::SparkSettingsStatus::Error(err.clone())
        } else {
            crate::app::view::spark::SparkSettingsStatus::Loading
        };

        crate::app::view::dashboard(
            menu,
            cache,
            SparkSettingsView {
                status,
                network: cache.network,
                default_lightning_backend: cache.default_lightning_backend,
                spark_available: self.backend.is_some(),
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
            return Task::none();
        };
        self.loading = true;
        self.error = None;
        let info_task = Task::perform(
            {
                let backend = backend.clone();
                async move { backend.get_info().await }
            },
            |result: Result<GetInfoOk, _>| match result {
                Ok(info) => Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::DataLoaded(SparkSettingsSnapshot {
                        balance_sats: info.balance_sats,
                        identity_pubkey: info.identity_pubkey,
                    }),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::Error(e.to_string()),
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
                    // Swallow into a no-op — the Stable Balance
                    // toggle just stays in its "loading" state
                    // and the rest of the panel still works.
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
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(crate::app::view::Message::SparkSettings(msg)) = message {
            match msg {
                crate::app::view::SparkSettingsMessage::DataLoaded(snapshot) => {
                    self.loading = false;
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                crate::app::view::SparkSettingsMessage::Error(err) => {
                    self.loading = false;
                    self.error = Some(err);
                }
                crate::app::view::SparkSettingsMessage::DefaultLightningBackendChanged(kind) => {
                    // Persist asynchronously. On success, emit
                    // SettingsSaved so App reloads cube_settings
                    // and cache from disk.
                    let datadir = cache.datadir_path.clone();
                    let network = cache.network;
                    let cube_id = cache.cube_id.clone();
                    return Task::perform(
                        async move {
                            use crate::app::settings::update_settings_file;
                            let network_dir = datadir.network_directory(network);
                            update_settings_file(&network_dir, |mut settings| {
                                if let Some(cube) =
                                    settings.cubes.iter_mut().find(|c| c.id == cube_id)
                                {
                                    cube.default_lightning_backend = kind;
                                } else {
                                    tracing::error!(
                                        "Cube not found (id={}) — cannot save default_lightning_backend",
                                        cube_id
                                    );
                                }
                                Some(settings)
                            })
                            .await
                            .map_err(|e| e.to_string())
                        },
                        |result| match result {
                            Ok(()) => Message::SettingsSaved,
                            Err(err) => Message::View(
                                crate::app::view::Message::SparkSettings(
                                    crate::app::view::SparkSettingsMessage::DefaultLightningBackendSaved(
                                        Some(err),
                                    ),
                                ),
                            ),
                        },
                    );
                }
                crate::app::view::SparkSettingsMessage::DefaultLightningBackendSaved(err) => {
                    if let Some(err) = err {
                        tracing::warn!("Failed to persist default_lightning_backend: {}", err);
                        self.error = Some(err);
                    }
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
                            Ok(()) => Message::View(
                                crate::app::view::Message::SparkSettings(
                                    crate::app::view::SparkSettingsMessage::StableBalanceSaved(
                                        Ok(enabled),
                                    ),
                                ),
                            ),
                            Err(e) => Message::View(
                                crate::app::view::Message::SparkSettings(
                                    crate::app::view::SparkSettingsMessage::StableBalanceSaved(
                                        Err(e.to_string()),
                                    ),
                                ),
                            ),
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
                            // Revert optimistic update by flipping
                            // back to the previous state (whatever
                            // value is not the attempted one).
                            if let Some(current) = self.stable_balance_active {
                                self.stable_balance_active = Some(!current);
                            }
                            self.error = Some(err);
                        }
                    }
                }
            }
        }
        Task::none()
    }
}
