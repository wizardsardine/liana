use std::sync::Arc;

use iced::{clipboard, Task};
use liana_ui::{component::form, widget::Element};

use lianad::config::{Config as DaemonConfig, PayjoinConfig};

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::settings::State, view},
    daemon::Daemon,
};

#[derive(Debug)]
pub struct PayjoinSettingsState {
    warning: Option<Error>,
    config_updated: bool,
    payjoin_settings: PayjoinSettings,
}

impl PayjoinSettingsState {
    pub fn new(config: Option<DaemonConfig>) -> Self {
        let payjoin_config =
            config
                .and_then(|c| c.payjoin_config)
                .unwrap_or_else(|| PayjoinConfig {
                    ohttp_relay: "https://pj.bobspacebkk.com".to_string(),
                    payjoin_directory: "https://payjo.in".to_string(),
                });
        PayjoinSettingsState {
            warning: None,
            config_updated: false,
            payjoin_settings: PayjoinSettings::new(payjoin_config),
        }
    }
}

impl State for PayjoinSettingsState {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::DaemonConfigLoaded(res) => match res {
                Ok(()) => {
                    self.config_updated = true;
                    self.warning = None;
                    self.payjoin_settings.edited(true);
                    return Task::perform(async {}, |_| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::EditPayjoinSettings,
                        ))
                    });
                }
                Err(e) => {
                    self.config_updated = false;
                    self.warning = Some(e);
                    self.payjoin_settings.edited(false);
                }
            },
            Message::View(view::Message::Settings(view::SettingsMessage::PayjoinSettings(msg))) => {
                return self.payjoin_settings.update(daemon, msg);
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::settings::payjoin_settings(
            cache,
            self.warning.as_ref(),
            Some(self.payjoin_settings.view(cache)),
        )
    }
}

impl From<PayjoinSettingsState> for Box<dyn State> {
    fn from(s: PayjoinSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Debug)]
pub struct PayjoinSettings {
    edit: bool,
    processing: bool,
    ohttp_relay: form::Value<String>,
    payjoin_directory: form::Value<String>,
}

impl PayjoinSettings {
    pub fn new(config: PayjoinConfig) -> Self {
        PayjoinSettings {
            edit: false,
            processing: false,
            ohttp_relay: form::Value {
                valid: true,
                warning: None,
                value: config.ohttp_relay,
            },
            payjoin_directory: form::Value {
                valid: true,
                warning: None,
                value: config.payjoin_directory,
            },
        }
    }
}

impl PayjoinSettings {
    fn edited(&mut self, success: bool) {
        self.processing = false;
        if success {
            self.edit = false;
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: view::SettingsEditMessage,
    ) -> Task<Message> {
        match message {
            view::SettingsEditMessage::Select => {
                if !self.processing {
                    self.edit = true;
                }
            }
            view::SettingsEditMessage::Cancel => {
                if !self.processing {
                    self.edit = false;
                }
            }
            view::SettingsEditMessage::FieldEdited(field, value) => {
                if !self.processing {
                    match field {
                        "ohttp_relay" => self.ohttp_relay.value = value,
                        "payjoin_directory" => self.payjoin_directory.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsEditMessage::Confirm => {
                let mut daemon_config = daemon.config().cloned().unwrap();
                daemon_config.payjoin_config = Some(PayjoinConfig::new(
                    self.ohttp_relay.value.clone(),
                    self.payjoin_directory.value.clone(),
                ));
                self.processing = true;
                return Task::perform(async move { daemon_config }, |cfg| {
                    Message::LoadDaemonConfig(Box::new(cfg))
                });
            }
            view::SettingsEditMessage::Clipboard(text) => return clipboard::write(text),
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&self, cache: &'a Cache) -> Element<'a, view::SettingsEditMessage> {
        if self.edit {
            view::settings::payjoin_edit(
                cache,
                &self.ohttp_relay,
                &self.payjoin_directory,
                self.processing,
            )
        } else {
            view::settings::payjoin(
                cache,
                &self.ohttp_relay.value,
                &self.payjoin_directory.value,
            )
        }
    }
}
