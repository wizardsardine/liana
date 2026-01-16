use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::error::Error;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::{Daemon, DaemonBackend};

#[derive(Default)]
pub struct AboutSettingsState {
    daemon_version: Option<String>,
    warning: Option<Error>,
}

impl State for AboutSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::about::about_section(
            menu,
            cache,
            None, // Errors now shown via global toast
            self.daemon_version.as_ref(),
        )
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::Info(res) = message {
            match res {
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
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            }
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
