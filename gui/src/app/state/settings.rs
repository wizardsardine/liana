use std::convert::From;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use iced::{pure::Element, Command};

use minisafe::config::Config;

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::State, view},
    daemon::Daemon,
    ui::component::form,
};

trait Setting: std::fmt::Debug {
    fn edited(&mut self, success: bool);
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: view::SettingsMessage,
    ) -> Command<Message>;
    fn view<'a>(
        &self,
        cfg: &'a Config,
        cache: &'a Cache,
        can_edit: bool,
    ) -> Element<'a, view::SettingsMessage>;
}

#[derive(Debug)]
pub struct SettingsState {
    warning: Option<Error>,
    config_updated: bool,
    config: Config,
    daemon_is_external: bool,

    settings: Vec<Box<dyn Setting>>,
    current: Option<usize>,
}

impl SettingsState {
    pub fn new(config: Config, daemon_is_external: bool) -> Self {
        let settings = vec![BitcoindSettings::new(&config).into()];

        SettingsState {
            daemon_is_external,
            warning: None,
            config_updated: false,
            config,
            settings,
            current: None,
        }
    }
}

impl State for SettingsState {
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
            Message::View(view::Message::Settings(i, msg)) => {
                if let Some(setting) = self.settings.get_mut(i) {
                    match msg {
                        view::SettingsMessage::Edit => self.current = Some(i),
                        view::SettingsMessage::CancelEdit => self.current = None,
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
        view::settings::list(
            cache,
            self.warning.as_ref(),
            self.settings
                .iter()
                .enumerate()
                .map(|(i, setting)| {
                    setting
                        .view(&self.config, cache, can_edit)
                        .map(move |msg| view::Message::Settings(i, msg))
                })
                .collect(),
        )
    }
}

impl From<SettingsState> for Box<dyn State> {
    fn from(s: SettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Debug)]
pub struct BitcoindSettings {
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
    fn new(cfg: &Config) -> BitcoindSettings {
        let cfg = cfg.bitcoind_config.as_ref().unwrap();
        BitcoindSettings {
            edit: false,
            processing: false,
            cookie_path: form::Value {
                valid: true,
                value: cfg.cookie_path.to_str().unwrap().to_string(),
            },
            addr: form::Value {
                valid: true,
                value: cfg.addr.to_string(),
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
        message: view::SettingsMessage,
    ) -> Command<Message> {
        match message {
            view::SettingsMessage::Edit => {
                if !self.processing {
                    self.edit = true;
                }
            }
            view::SettingsMessage::CancelEdit => {
                if !self.processing {
                    self.edit = false;
                }
            }
            view::SettingsMessage::FieldEdited(field, value) => {
                if !self.processing {
                    match field {
                        "socket_address" => self.addr.value = value,
                        "cookie_file_path" => self.cookie_path.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsMessage::ConfirmEdit => {
                let new_addr = SocketAddr::from_str(&self.addr.value);
                self.addr.valid = new_addr.is_ok();
                let new_path = PathBuf::from_str(&self.cookie_path.value);
                self.cookie_path.valid = new_path.is_ok();

                if self.addr.valid & self.cookie_path.valid {
                    let mut daemon_config = daemon.config().clone();
                    daemon_config.bitcoind_config = Some(minisafe::config::BitcoindConfig {
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

    fn view<'a>(
        &self,
        config: &'a Config,
        cache: &'a Cache,
        can_edit: bool,
    ) -> Element<'a, view::SettingsMessage> {
        if self.edit {
            view::settings::bitcoind_edit(
                config.bitcoin_config.network,
                cache.blockheight,
                &self.addr,
                &self.cookie_path,
                self.processing,
            )
        } else {
            view::settings::bitcoind(
                config.bitcoin_config.network,
                config.bitcoind_config.as_ref().unwrap(),
                cache.blockheight,
                Some(cache.blockheight != 0),
                can_edit,
            )
        }
    }
}
