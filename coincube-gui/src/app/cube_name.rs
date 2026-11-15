// Cube-name persistence and reconciliation remain one ordered hand-off.
use super::*;

impl App {
    pub(super) fn route_cube_name_submitted(&mut self, name: String) -> Task<Message> {
        let name = name.trim().to_owned();
        if name.is_empty() || name == self.cube_settings.name {
            let daemon = self.daemon.clone();
            return self.panels.global_settings.update(
                daemon,
                &self.cache,
                Message::View(view::Message::Settings(
                    view::SettingsMessage::CubeNameSubmitted(name),
                )),
            );
        }
        let network_dir = self.datadir.network_directory(self.cube_settings.network);
        let cube_id = self.cube_settings.id.clone();
        Task::perform(
            async move {
                let saved = name.clone();
                settings::update_settings_file(&network_dir, move |mut s| {
                    if let Some(c) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                        c.name = saved;
                        c.remote_synced = false;
                    }
                    Some(s)
                })
                .await
                .map(|_| name)
                .map_err(|e| e.to_string())
            },
            Message::CubeNameSaved,
        )
    }

    pub(super) fn route_cube_name_saved(
        &mut self,
        result: Result<String, String>,
    ) -> Task<Message> {
        let name = match result {
            Ok(name) => name,
            Err(e) => {
                return Task::done(Message::View(view::Message::ShowError(
                    coincube_ui::localized_format!(
                        "Couldn't save the Cube name: {}",
                        "No se pudo guardar el nombre del Cube: {}",
                        e
                    ),
                )));
            }
        };
        self.cube_settings.name = name.clone();
        self.cache.cube_name = name.clone();
        // A later registration retry submits the panel's name.
        self.panels.connect.cube.cube_name = name.clone();
        let daemon = self.daemon.clone();
        let settle = self.panels.global_settings.update(
            daemon,
            &self.cache,
            Message::View(view::Message::Settings(
                view::SettingsMessage::CubeNameSubmitted(name.clone()),
            )),
        );
        let (Some(client), Some(server_id)) = (
            self.authenticated_coincube_client(),
            self.panels.connect.cube.server_cube_id,
        ) else {
            return settle;
        };
        let cube_id = self.cube_settings.id.clone();
        let network_dir = self.datadir.network_directory(self.cube_settings.network);
        let synced_name = name.clone();
        let request = crate::services::coincube::UpdateCubeRequest {
            name: Some(name),
            status: None,
            // Name-only rename: leave server Vault presence and the
            // Spark Stable Balance record untouched.
            has_vault: None,
            spark_stable_balance: None,
        };
        let remote = Task::perform(
            {
                let cube_id = cube_id.clone();
                async move {
                    client
                        .update_cube(&server_id.to_string(), request)
                        .await
                        .map_err(|e| e.to_string())?;
                    settings::update_settings_file(&network_dir, move |mut s| {
                        // A newer rename may have been written while
                        // this one was in flight; leave it unsynced.
                        if let Some(c) = s
                            .cubes
                            .iter_mut()
                            .find(|c| c.id == cube_id && c.name == synced_name)
                        {
                            c.remote_synced = true;
                        }
                        Some(s)
                    })
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                }
            },
            move |result| Message::CubeNameSynced {
                cube_id: cube_id.clone(),
                result,
            },
        );
        Task::batch([settle, remote])
    }

    pub(super) fn route_cube_name_synced(
        &mut self,
        cube_id: String,
        result: Result<(), String>,
    ) -> Task<Message> {
        // Left unsynced on failure; the Launcher's catch-up retries it.
        if let Err(e) = result {
            warn!("Cube {} rename not yet synced to Connect: {}", cube_id, e);
        }
        Task::none()
    }
}
