use std::convert::From;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::prelude::*;
use iced::Command;
use tracing::info;

use liana::config::{BitcoinConfig, BitcoindConfig, Config};

use liana_ui::{component::form, widget::Element};

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::settings::Setting, view, State},
    daemon::Daemon,
};

#[derive(Debug)]
pub struct BitcoindSettingsState {
    warning: Option<Error>,
    config_updated: bool,
    daemon_is_external: bool,

    settings: Vec<Box<dyn Setting>>,
    current: Option<usize>,
}

impl BitcoindSettingsState {
    pub fn new(config: Option<Config>, cache: &Cache, daemon_is_external: bool) -> Self {
        let settings = if let Some(config) = &config {
            vec![
                BitcoindSettings::new(
                    config.bitcoin_config.clone(),
                    config.bitcoind_config.clone().unwrap(),
                )
                .into(),
                RescanSetting::new(cache.rescan_progress).into(),
            ]
        } else {
            vec![RescanSetting::new(cache.rescan_progress).into()]
        };

        BitcoindSettingsState {
            daemon_is_external,
            warning: None,
            config_updated: false,
            settings,
            // If a scan is running, the current setting edited is the Rescan panel.
            current: cache.rescan_progress.map(|_| 1),
        }
    }
}

impl State for BitcoindSettingsState {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::DaemonConfigLoaded(res) => match res {
                Ok(()) => {
                    self.config_updated = true;
                    self.warning = None;
                    if let Some(current) = self.current {
                        if let Some(setting) = self.settings.get_mut(current) {
                            setting.edited(true)
                        }
                    }
                    self.current = None;
                }
                Err(e) => {
                    self.config_updated = false;
                    self.warning = Some(e);
                    if let Some(current) = self.current {
                        if let Some(setting) = self.settings.get_mut(current) {
                            setting.edited(false);
                        }
                    }
                }
            },
            Message::Info(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(info) => {
                    if info.rescan_progress == Some(1.0) {
                        self.settings[1].edited(true);
                    }
                }
            },
            Message::View(view::Message::Settings(view::SettingsMessage::Edit(i, msg))) => {
                if let Some(setting) = self.settings.get_mut(i) {
                    match msg {
                        view::SettingsEditMessage::Select => self.current = Some(i),
                        view::SettingsEditMessage::Cancel => self.current = None,
                        _ => {}
                    };
                    return setting.update(daemon, cache, msg);
                }
            }
            _ => {}
        };
        Command::none()
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let can_edit = self.current.is_none() && !self.daemon_is_external;
        view::settings::bitcoind_settings(
            cache,
            self.warning.as_ref(),
            self.settings
                .iter()
                .enumerate()
                .map(|(i, setting)| {
                    setting.view(cache, can_edit).map(move |msg| {
                        view::Message::Settings(view::SettingsMessage::Edit(i, msg))
                    })
                })
                .collect(),
        )
    }
}

impl From<BitcoindSettingsState> for Box<dyn State> {
    fn from(s: BitcoindSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Debug)]
pub struct BitcoindSettings {
    bitcoind_config: BitcoindConfig,
    bitcoin_config: BitcoinConfig,
    edit: bool,
    processing: bool,
    cookie_path: form::Value<String>,
    addr: form::Value<String>,
}

impl From<BitcoindSettings> for Box<dyn Setting> {
    fn from(s: BitcoindSettings) -> Box<dyn Setting> {
        Box::new(s)
    }
}

impl BitcoindSettings {
    fn new(bitcoin_config: BitcoinConfig, bitcoind_config: BitcoindConfig) -> BitcoindSettings {
        let path = bitcoind_config.cookie_path.to_str().unwrap().to_string();
        let addr = bitcoind_config.addr.to_string();
        BitcoindSettings {
            bitcoind_config,
            bitcoin_config,
            edit: false,
            processing: false,
            cookie_path: form::Value {
                valid: true,
                value: path,
            },
            addr: form::Value {
                valid: true,
                value: addr,
            },
        }
    }
}

impl Setting for BitcoindSettings {
    fn edited(&mut self, success: bool) {
        self.processing = false;
        if success {
            self.edit = false;
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: view::SettingsEditMessage,
    ) -> Command<Message> {
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
                        "socket_address" => self.addr.value = value,
                        "cookie_file_path" => self.cookie_path.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsEditMessage::Confirm => {
                let new_addr = SocketAddr::from_str(&self.addr.value);
                self.addr.valid = new_addr.is_ok();
                let new_path = PathBuf::from_str(&self.cookie_path.value);
                self.cookie_path.valid = new_path.is_ok();

                if self.addr.valid & self.cookie_path.valid {
                    let mut daemon_config = daemon.config().cloned().unwrap();
                    daemon_config.bitcoind_config = Some(liana::config::BitcoindConfig {
                        cookie_path: new_path.unwrap(),
                        addr: new_addr.unwrap(),
                    });
                    self.processing = true;
                    return Command::perform(async move { daemon_config }, |cfg| {
                        Message::LoadDaemonConfig(Box::new(cfg))
                    });
                }
            }
        };
        Command::none()
    }

    fn view<'a>(&self, cache: &'a Cache, can_edit: bool) -> Element<'a, view::SettingsEditMessage> {
        if self.edit {
            view::settings::bitcoind_edit(
                self.bitcoin_config.network,
                cache.blockheight,
                &self.addr,
                &self.cookie_path,
                self.processing,
            )
        } else {
            view::settings::bitcoind(
                self.bitcoin_config.network,
                &self.bitcoind_config,
                cache.blockheight,
                Some(cache.blockheight != 0),
                can_edit,
            )
        }
    }
}

#[derive(Debug, Default)]
pub struct RescanSetting {
    edit: bool,
    processing: bool,
    success: bool,
    year: form::Value<String>,
    month: form::Value<String>,
    day: form::Value<String>,
}

impl From<RescanSetting> for Box<dyn Setting> {
    fn from(s: RescanSetting) -> Box<dyn Setting> {
        Box::new(s)
    }
}

impl RescanSetting {
    pub fn new(rescan_progress: Option<f64>) -> Self {
        Self {
            processing: rescan_progress.is_some(),
            ..Default::default()
        }
    }
}

impl Setting for RescanSetting {
    fn edited(&mut self, success: bool) {
        self.processing = false;
        self.success = success;
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: view::SettingsEditMessage,
    ) -> Command<Message> {
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
                if !self.processing && (value.is_empty() || u32::from_str(&value).is_ok()) {
                    match field {
                        "rescan_year" => self.year.value = value,
                        "rescan_month" => self.month.value = value,
                        "rescan_day" => self.day.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsEditMessage::Confirm => {
                let date_time = NaiveDate::from_ymd(
                    i32::from_str(&self.year.value).unwrap_or(1),
                    u32::from_str(&self.month.value).unwrap_or(1),
                    u32::from_str(&self.day.value).unwrap_or(1),
                )
                .and_hms(0, 0, 0);
                let t = date_time.timestamp() as u32;
                self.processing = true;
                info!("Asking deamon to rescan with timestamp: {}", t);
                return Command::perform(
                    async move { daemon.start_rescan(t).map_err(|e| e.into()) },
                    Message::StartRescan,
                );
            }
        };
        Command::none()
    }

    fn view<'a>(&self, cache: &'a Cache, can_edit: bool) -> Element<'a, view::SettingsEditMessage> {
        view::settings::rescan(
            &self.year,
            &self.month,
            &self.day,
            cache.rescan_progress,
            self.success,
            self.processing,
            can_edit,
        )
    }
}
