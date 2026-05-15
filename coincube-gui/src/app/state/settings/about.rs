use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::{Daemon, DaemonBackend};

#[derive(Default)]
pub struct AboutSettingsState {
    daemon_version: Option<String>,
    /// Status of the most recent "Re-register this device" click.
    /// Drives the inline banner under the Connect device card. `None`
    /// means "no recent attempt"; `Ok(_)` shows a green confirmation
    /// with the new device id; `Err(_)` shows the API error so the
    /// user can decide whether to retry.
    pub reregister_status: Option<Result<String, String>>,
    /// True while a re-register RPC is in flight. Disables the button
    /// to prevent double-clicks.
    pub reregistering: bool,
}

impl State for AboutSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::about::about_section(
            menu,
            cache,
            self.daemon_version.as_ref(),
            self,
        )
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::Info(res) => match res {
                Ok(info) => {
                    if let Some(daemon) = daemon {
                        if daemon.backend() == DaemonBackend::RemoteBackend {
                            self.daemon_version = None;
                        } else {
                            self.daemon_version = Some(info.version)
                        }
                    } else {
                        self.daemon_version = None;
                    }
                }
                Err(e) => {
                    return Task::done(Message::View(view::Message::ShowError(e.to_string())));
                }
            },
            Message::View(view::Message::Settings(
                view::SettingsMessage::ReregisterConnectDevice,
            )) => {
                // Read the bits we need to spawn the RPC. Without
                // grpc_url and tokens there's nothing we can do —
                // surface a clear error rather than silently swallow
                // the click.
                let (Some(grpc_url), Some(tokens), Some(email)) = (
                    cache.connect_grpc_url.clone(),
                    cache.connect_tokens.clone(),
                    cache.connect_email.clone(),
                ) else {
                    self.reregister_status = Some(Err(
                        "Connect isn't ready yet. Wait a moment, then try again.".to_string(),
                    ));
                    return Task::none();
                };
                let network_dir = cache.datadir_path.network_directory(cache.network);
                self.reregistering = true;
                self.reregister_status = None;
                let app_version = env!("CARGO_PKG_VERSION").to_string();
                let os_version = std::env::consts::OS.to_string();
                // Match the device label format used at first launch
                // (see `services/connect/login.rs::device_name_for_this_host`)
                // so re-register produces a consistent name.
                let device_name = std::env::var("HOSTNAME")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty()))
                    .unwrap_or_else(|| format!("Coincube Desktop ({})", std::env::consts::OS));
                return Task::perform(
                    async move {
                        // Clear the cached id so `ensure_device_registered`
                        // takes the register path instead of short-circuiting
                        // on the existing entry.
                        if let Err(e) =
                            crate::services::connect::client::cache::set_device_id_for_email(
                                &network_dir,
                                &email,
                                None,
                            )
                            .await
                        {
                            // Best-effort: a cache write failure here
                            // just means the next launch may still
                            // hit the existing id. Don't block the
                            // RPC on it.
                            tracing::warn!("Re-register: failed to clear cached device_id: {}", e,);
                        }
                        crate::services::connect::grpc::bootstrap::ensure_device_registered(
                            &grpc_url,
                            tokens,
                            &network_dir,
                            &email,
                            device_name,
                            app_version,
                            os_version,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    },
                    |r| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::ConnectDeviceReregistered(r),
                        ))
                    },
                );
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ConnectDeviceReregistered(res),
            )) => {
                self.reregistering = false;
                self.reregister_status = Some(res);
            }
            _ => {}
        }

        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        if let Some(daemon) = daemon {
            Task::perform(
                async move { daemon.get_info().await.map_err(|e| e.into()) },
                Message::Info,
            )
        } else {
            Task::none()
        }
    }
}

impl From<AboutSettingsState> for Box<dyn State> {
    fn from(s: AboutSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}
