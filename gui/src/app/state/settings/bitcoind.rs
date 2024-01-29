use std::convert::From;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::prelude::*;
use iced::Command;
use tracing::info;

use liana::config::{BitcoinConfig, BitcoindConfig, BitcoindRpcAuth, Config};

use liana_ui::{component::form, widget::Element};

use crate::{
    app::{cache::Cache, error::Error, message::Message, view, State},
    bitcoind::{RpcAuthType, RpcAuthValues},
    daemon::Daemon,
};

#[derive(Debug)]
pub struct BitcoindSettingsState {
    warning: Option<Error>,
    config_updated: bool,

    node_settings: Option<BitcoindSettings>,
    rescan_settings: RescanSetting,
}

impl BitcoindSettingsState {
    pub fn new(
        config: Option<Config>,
        cache: &Cache,
        daemon_is_external: bool,
        bitcoind_is_internal: bool,
    ) -> Self {
        BitcoindSettingsState {
            warning: None,
            config_updated: false,
            node_settings: config.map(|config| {
                BitcoindSettings::new(
                    config.bitcoin_config.clone(),
                    config.bitcoind_config.clone().unwrap(),
                    daemon_is_external,
                    bitcoind_is_internal,
                )
            }),
            rescan_settings: RescanSetting::new(cache.rescan_progress),
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
                    if let Some(settings) = &mut self.node_settings {
                        settings.edited(true);
                        return Command::perform(async {}, |_| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::EditBitcoindSettings,
                            ))
                        });
                    }
                }
                Err(e) => {
                    self.config_updated = false;
                    self.warning = Some(e);
                    if let Some(settings) = &mut self.node_settings {
                        settings.edited(false);
                    }
                }
            },
            Message::Info(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(info) => {
                    if info.rescan_progress == Some(1.0) {
                        self.rescan_settings.edited(true);
                    }
                }
            },
            Message::StartRescan(Err(_)) => {
                self.rescan_settings.past_possible_height = true;
                self.rescan_settings.processing = false;
            }
            Message::View(view::Message::Settings(view::SettingsMessage::BitcoindSettings(
                msg,
            ))) => {
                if let Some(settings) = &mut self.node_settings {
                    return settings.update(daemon, cache, msg);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RescanSettings(msg))) => {
                return self.rescan_settings.update(daemon, cache, msg);
            }
            _ => {}
        };
        Command::none()
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let can_edit_bitcoind_settings = !self.rescan_settings.processing;
        let can_do_rescan = !self.rescan_settings.processing
            && self.node_settings.as_ref().map(|settings| settings.edit) != Some(true);
        view::settings::bitcoind_settings(
            cache,
            self.warning.as_ref(),
            if let Some(settings) = &self.node_settings {
                vec![
                    settings
                        .view(cache, can_edit_bitcoind_settings)
                        .map(move |msg| {
                            view::Message::Settings(view::SettingsMessage::BitcoindSettings(msg))
                        }),
                    self.rescan_settings
                        .view(cache, can_do_rescan)
                        .map(move |msg| {
                            view::Message::Settings(view::SettingsMessage::RescanSettings(msg))
                        }),
                ]
            } else {
                vec![self
                    .rescan_settings
                    .view(cache, can_do_rescan)
                    .map(move |msg| {
                        view::Message::Settings(view::SettingsMessage::RescanSettings(msg))
                    })]
            },
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
    rpc_auth_vals: RpcAuthValues,
    selected_auth_type: RpcAuthType,
    addr: form::Value<String>,
    daemon_is_external: bool,
    bitcoind_is_internal: bool,
}

impl BitcoindSettings {
    fn new(
        bitcoin_config: BitcoinConfig,
        bitcoind_config: BitcoindConfig,
        daemon_is_external: bool,
        bitcoind_is_internal: bool,
    ) -> BitcoindSettings {
        let (rpc_auth_vals, selected_auth_type) = match &bitcoind_config.rpc_auth {
            BitcoindRpcAuth::CookieFile(path) => (
                RpcAuthValues {
                    cookie_path: form::Value {
                        valid: true,
                        value: path.to_str().unwrap().to_string(),
                    },
                    user: form::Value::default(),
                    password: form::Value::default(),
                },
                RpcAuthType::CookieFile,
            ),
            BitcoindRpcAuth::UserPass(user, password) => (
                RpcAuthValues {
                    cookie_path: form::Value::default(),
                    user: form::Value {
                        valid: true,
                        value: user.clone(),
                    },
                    password: form::Value {
                        valid: true,
                        value: password.clone(),
                    },
                },
                RpcAuthType::UserPass,
            ),
        };
        let addr = bitcoind_config.addr.to_string();
        BitcoindSettings {
            daemon_is_external,
            bitcoind_is_internal,
            bitcoind_config,
            bitcoin_config,
            edit: false,
            processing: false,
            rpc_auth_vals,
            selected_auth_type,
            addr: form::Value {
                valid: true,
                value: addr,
            },
        }
    }
}

impl BitcoindSettings {
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
                        "cookie_file_path" => self.rpc_auth_vals.cookie_path.value = value,
                        "user" => self.rpc_auth_vals.user.value = value,
                        "password" => self.rpc_auth_vals.password.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsEditMessage::BitcoindRpcAuthTypeSelected(auth_type) => {
                if !self.processing {
                    self.selected_auth_type = auth_type;
                }
            }
            view::SettingsEditMessage::Confirm => {
                let new_addr = SocketAddr::from_str(&self.addr.value);
                self.addr.valid = new_addr.is_ok();
                let rpc_auth = match self.selected_auth_type {
                    RpcAuthType::CookieFile => {
                        let new_path = PathBuf::from_str(&self.rpc_auth_vals.cookie_path.value);
                        if let Ok(path) = new_path {
                            self.rpc_auth_vals.cookie_path.valid = true;
                            Some(BitcoindRpcAuth::CookieFile(path))
                        } else {
                            None
                        }
                    }
                    RpcAuthType::UserPass => Some(BitcoindRpcAuth::UserPass(
                        self.rpc_auth_vals.user.value.clone(),
                        self.rpc_auth_vals.password.value.clone(),
                    )),
                };

                if let (true, Some(rpc_auth)) = (self.addr.valid, rpc_auth) {
                    let mut daemon_config = daemon.config().cloned().unwrap();
                    daemon_config.bitcoind_config = Some(liana::config::BitcoindConfig {
                        rpc_auth,
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
                &self.rpc_auth_vals,
                &self.selected_auth_type,
                self.processing,
            )
        } else {
            view::settings::bitcoind(
                self.bitcoin_config.network,
                &self.bitcoind_config,
                cache.blockheight,
                Some(cache.blockheight != 0),
                can_edit && !self.daemon_is_external && !self.bitcoind_is_internal,
            )
        }
    }
}

#[derive(Debug, Default)]
pub struct RescanSetting {
    processing: bool,
    success: bool,
    year: form::Value<String>,
    month: form::Value<String>,
    day: form::Value<String>,
    invalid_date: bool,
    past_possible_height: bool,
}

impl RescanSetting {
    pub fn new(rescan_progress: Option<f64>) -> Self {
        Self {
            processing: if let Some(progress) = rescan_progress {
                progress < 1.0
            } else {
                false
            },
            ..Default::default()
        }
    }
}

impl RescanSetting {
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
            view::SettingsEditMessage::FieldEdited(field, value) => {
                self.invalid_date = false;
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
                let date_time = if let Some(date) = NaiveDate::from_ymd_opt(
                    i32::from_str(&self.year.value).unwrap_or(1),
                    u32::from_str(&self.month.value).unwrap_or(1),
                    u32::from_str(&self.day.value).unwrap_or(1),
                ) {
                    if date < NaiveDate::from_str("2009-01-03").unwrap() {
                        self.past_possible_height = true;
                        return Command::none();
                    } else {
                        self.past_possible_height = false;
                        date
                    }
                } else {
                    self.invalid_date = true;
                    return Command::none();
                };
                let t = date_time.and_hms_opt(0, 0, 0).unwrap().timestamp() as u32;
                self.processing = true;
                info!("Asking deamon to rescan with timestamp: {}", t);
                return Command::perform(
                    async move { daemon.start_rescan(t).map_err(|e| e.into()) },
                    Message::StartRescan,
                );
            }
            _ => {}
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
            self.invalid_date,
            self.past_possible_height,
        )
    }
}
