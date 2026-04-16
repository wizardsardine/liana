//! State panel for **Settings → Lightning**.
//!
//! Thin wrapper around the view in
//! [`crate::app::view::settings::lightning`]. Owns nothing but the
//! cube id it needs to locate the right entry in the settings file
//! when the picker fires. The actual picker state lives on
//! [`Cache::default_lightning_backend`], which is the app's mirror
//! of `CubeSettings::default_lightning_backend` — the
//! authoritative copy on disk is updated through
//! [`crate::app::settings::update_settings_file`], and `App` re-reads
//! it on `Message::SettingsSaved`.

use std::sync::Arc;

use iced::Task;

use coincube_ui::widget::Element;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::settings::update_settings_file;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;

pub struct LightningSettingsState {
    cube_id: String,
}

impl LightningSettingsState {
    pub fn new(cube_id: String) -> Self {
        Self { cube_id }
    }
}

impl From<LightningSettingsState> for Box<dyn State> {
    fn from(s: LightningSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl State for LightningSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::lightning::lightning_section(menu, cache)
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        Task::none()
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::Settings(
            view::SettingsMessage::DefaultLightningBackendChanged(kind),
        )) = message
        {
            // Persist asynchronously. `Message::SettingsSaved` is the
            // app-level signal that cube_settings + cache should be
            // re-read from disk — the App handler at
            // [`crate::app::App::update`] does that for us.
            let datadir = cache.datadir_path.clone();
            let network = cache.network;
            let cube_id = self.cube_id.clone();
            return Task::perform(
                async move {
                    let network_dir = datadir.network_directory(network);
                    let mut cube_found = false;
                    update_settings_file(&network_dir, |mut settings| {
                        if let Some(entry) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
                            entry.default_lightning_backend = kind;
                            cube_found = true;
                        }
                        Some(settings)
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                    if cube_found {
                        Ok(())
                    } else {
                        Err(format!(
                            "Cube not found (id={}) — cannot save default_lightning_backend",
                            cube_id
                        ))
                    }
                },
                |result| match result {
                    Ok(()) => Message::SettingsSaved,
                    Err(err) => {
                        tracing::warn!("Failed to persist default_lightning_backend: {}", err);
                        Message::View(view::Message::ShowError(format!(
                            "Failed to save Lightning backend: {}",
                            err
                        )))
                    }
                },
            );
        }
        Task::none()
    }
}
