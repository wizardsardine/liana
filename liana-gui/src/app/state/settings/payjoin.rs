use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, Subscription, Task};
use liana_ui::{component::form, widget::Element};

use lianad::config::{Config as DaemonConfig, PayjoinConfig};
use lianad::payjoin::Url;

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::settings::State, view},
    daemon::Daemon,
    utils::default_payjoin_config,
};

async fn check_relay_health(relay_url: &str, directory_url: &str) -> bool {
    let proxy = match reqwest::Proxy::https(relay_url) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let target = format!(
        "{}/.well-known/ohttp-gateway",
        directory_url.trim_end_matches('/')
    );
    client
        .get(&target)
        .send()
        .await
        .ok()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_directory_health(directory_url: &str) -> bool {
    let target = format!(
        "{}/.well-known/ohttp-gateway",
        directory_url.trim_end_matches('/')
    );
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(&target)
        .send()
        .await
        .ok()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[derive(Debug)]
pub struct PayjoinSettingsState {
    warning: Option<Error>,
    config_updated: bool,
    payjoin_settings: PayjoinSettings,
}

impl PayjoinSettingsState {
    pub fn new(config: Option<DaemonConfig>) -> (Self, Task<Message>) {
        let payjoin_config = config
            .and_then(|c| c.payjoin_config)
            .unwrap_or_else(default_payjoin_config);
        let state = PayjoinSettingsState {
            warning: None,
            config_updated: false,
            payjoin_settings: PayjoinSettings::new(payjoin_config),
        };
        let task = Task::perform(async {}, |_| {
            Message::View(view::Message::Settings(
                view::SettingsMessage::PayjoinSettings(view::SettingsEditMessage::HealthCheck),
            ))
        });
        (state, task)
    }
}

impl State for PayjoinSettingsState {
    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_secs(5)).map(|_| {
            Message::View(view::Message::Settings(
                view::SettingsMessage::PayjoinSettings(view::SettingsEditMessage::HealthCheck),
            ))
        })
    }

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
            Some(self.payjoin_settings.view()),
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
    ohttp_relays: Vec<form::Value<String>>,
    relay_health: Vec<Option<bool>>,
    payjoin_directory: form::Value<String>,
    directory_health: Option<bool>,
    original_config: Option<PayjoinConfig>,
}

impl PayjoinSettings {
    pub fn new(config: PayjoinConfig) -> Self {
        let relay_count = config.ohttp_relays.len();
        let ohttp_relays = if relay_count == 0 {
            vec![form::Value {
                valid: false,
                warning: None,
                value: String::new(),
            }]
        } else {
            config
                .ohttp_relays
                .into_iter()
                .map(|value| form::Value {
                    valid: is_valid_url(&value),
                    warning: None,
                    value,
                })
                .collect()
        };
        PayjoinSettings {
            edit: false,
            processing: false,
            relay_health: vec![Some(true); ohttp_relays.len()],
            ohttp_relays,
            directory_health: Some(true),
            payjoin_directory: form::Value {
                valid: is_valid_url(&config.payjoin_directory),
                warning: None,
                value: config.payjoin_directory,
            },
            original_config: None,
        }
    }
}

fn is_valid_url(s: &str) -> bool {
    Url::parse(s).is_ok_and(|u| u.scheme() == "http" || u.scheme() == "https")
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
                    self.original_config = Some(PayjoinConfig::new(
                        self.ohttp_relays
                            .iter()
                            .map(|r| r.value.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                        self.payjoin_directory.value.clone(),
                    ));
                    self.edit = true;
                }
            }
            view::SettingsEditMessage::Cancel => {
                if !self.processing {
                    if let Some(ref config) = self.original_config {
                        *self = Self::new(config.clone());
                    }
                    self.edit = false;
                }
            }
            view::SettingsEditMessage::FieldEdited(field, value) => {
                if !self.processing && field == "payjoin_directory" {
                    self.payjoin_directory.valid = is_valid_url(&value);
                    self.payjoin_directory.value = value;
                }
            }
            view::SettingsEditMessage::PayjoinRelayEdited(idx, value) => {
                if !self.processing {
                    if let Some(entry) = self.ohttp_relays.get_mut(idx) {
                        entry.valid = is_valid_url(&value);
                        entry.value = value;
                    }
                }
            }
            view::SettingsEditMessage::PayjoinRelayAdded => {
                if !self.processing {
                    self.ohttp_relays.push(form::Value {
                        valid: false,
                        warning: None,
                        value: String::new(),
                    });
                    self.relay_health.push(None);
                }
            }
            view::SettingsEditMessage::PayjoinRelayRemoved(idx) => {
                if !self.processing && self.ohttp_relays.len() > 1 {
                    self.ohttp_relays.remove(idx);
                    self.relay_health.remove(idx);
                }
            }
            view::SettingsEditMessage::Confirm => {
                let all_valid =
                    self.payjoin_directory.valid && self.ohttp_relays.iter().all(|r| r.valid);
                if !all_valid {
                    return Task::none();
                }
                let mut daemon_config = daemon.config().cloned().unwrap();
                let relays: Vec<String> = self
                    .ohttp_relays
                    .iter()
                    .map(|r| r.value.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if relays.is_empty() {
                    return Task::none();
                }
                daemon_config.payjoin_config = Some(PayjoinConfig::new(
                    relays,
                    self.payjoin_directory.value.clone(),
                ));
                self.processing = true;
                return Task::perform(async move { daemon_config }, |cfg| {
                    Message::LoadDaemonConfig(Box::new(cfg))
                });
            }
            view::SettingsEditMessage::Clipboard(text) => return clipboard::write(text),
            view::SettingsEditMessage::HealthCheck => {
                let mut tasks: Vec<Task<Message>> = Vec::new();
                let dir_url = self.payjoin_directory.value.clone();
                for (idx, relay) in self.ohttp_relays.iter().enumerate() {
                    if relay.value.is_empty() || dir_url.is_empty() {
                        continue;
                    }
                    let relay_url = relay.value.clone();
                    let dir = dir_url.clone();
                    tasks.push(Task::perform(
                        async move { check_relay_health(&relay_url, &dir).await },
                        move |ok| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::PayjoinSettings(
                                    view::SettingsEditMessage::HealthCheckRelayResult(idx, ok),
                                ),
                            ))
                        },
                    ));
                }
                if !dir_url.is_empty() {
                    tasks.push(Task::perform(
                        async move { check_directory_health(&dir_url).await },
                        |ok| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::PayjoinSettings(
                                    view::SettingsEditMessage::HealthCheckDirectoryResult(ok),
                                ),
                            ))
                        },
                    ));
                }
                if tasks.is_empty() {
                    return Task::none();
                }
                return Task::batch(tasks);
            }
            view::SettingsEditMessage::HealthCheckRelayResult(idx, ok) => {
                if let Some(entry) = self.relay_health.get_mut(idx) {
                    *entry = Some(ok);
                }
            }
            view::SettingsEditMessage::HealthCheckDirectoryResult(ok) => {
                self.directory_health = Some(ok);
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&self) -> Element<'a, view::SettingsEditMessage> {
        if self.edit {
            view::settings::payjoin_edit(
                &self.ohttp_relays,
                &self.relay_health,
                &self.payjoin_directory,
                self.directory_health,
                self.processing,
            )
        } else {
            let relays: Vec<String> = self.ohttp_relays.iter().map(|r| r.value.clone()).collect();
            view::settings::payjoin(
                &relays,
                &self.relay_health,
                &self.payjoin_directory.value,
                self.directory_health,
            )
        }
    }
}
