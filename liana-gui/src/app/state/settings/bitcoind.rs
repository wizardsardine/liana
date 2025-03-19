use std::convert::{From, TryInto};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use iced::{clipboard, Task};
use tracing::info;

use liana::miniscript::bitcoin::Network;
use lianad::config::{
    BitcoinBackend, BitcoinConfig, BitcoindConfig, BitcoindRpcAuth, Config, ElectrumConfig,
};

use liana_ui::{component::form, widget::Element};

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::settings::State, view},
    daemon::Daemon,
    node::{
        bitcoind::{RpcAuthType, RpcAuthValues},
        NodeType,
    },
};

#[derive(Debug)]
pub struct BitcoindSettingsState {
    warning: Option<Error>,
    config_updated: bool,

    bitcoind_settings: Option<BitcoindSettings>,
    electrum_settings: Option<ElectrumSettings>,
    rescan_settings: RescanSetting,
}

impl BitcoindSettingsState {
    pub fn new(
        config: Option<Config>,
        cache: &Cache,
        daemon_is_external: bool,
        bitcoind_is_internal: bool,
    ) -> Self {
        let mut configured_node_type = None;
        let (bitcoind_config, electrum_config) =
            match config.clone().and_then(|c| c.bitcoin_backend) {
                Some(BitcoinBackend::Bitcoind(bitcoind_config)) => {
                    configured_node_type = Some(NodeType::Bitcoind);
                    (Some(bitcoind_config), None)
                }
                Some(BitcoinBackend::Electrum(electrum_config)) => {
                    configured_node_type = Some(NodeType::Electrum);
                    (None, Some(electrum_config))
                }
                _ => (None, None),
            };
        BitcoindSettingsState {
            warning: None,
            config_updated: false,
            bitcoind_settings: bitcoind_config.map(|bitcoind_config| {
                BitcoindSettings::new(
                    configured_node_type,
                    config
                        .clone()
                        .expect("config must exist if bitcoind_config exists")
                        .bitcoin_config,
                    bitcoind_config,
                    daemon_is_external,
                    bitcoind_is_internal,
                )
            }),
            electrum_settings: electrum_config.map(|electrum_config| {
                ElectrumSettings::new(
                    configured_node_type,
                    config
                        .expect("config must exist if electrum_config exists")
                        .bitcoin_config,
                    electrum_config,
                    daemon_is_external,
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
    ) -> Task<Message> {
        match message {
            Message::DaemonConfigLoaded(res) => match res {
                Ok(()) => {
                    self.config_updated = true;
                    self.warning = None;
                    if let Some(settings) = &mut self.bitcoind_settings {
                        settings.edited(true);
                        return Task::perform(async {}, |_| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::EditBitcoindSettings,
                            ))
                        });
                    }
                    if let Some(settings) = &mut self.electrum_settings {
                        settings.edited(true);
                        return Task::perform(async {}, |_| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::EditBitcoindSettings,
                            ))
                        });
                    }
                }
                Err(e) => {
                    self.config_updated = false;
                    self.warning = Some(e);
                    if let Some(settings) = &mut self.bitcoind_settings {
                        settings.edited(false);
                    }
                    if let Some(settings) = &mut self.electrum_settings {
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
            Message::UpdatePanelCache(_) => {
                self.rescan_settings.processing = cache.rescan_progress.is_some_and(|p| p < 1.0);
            }
            Message::View(view::Message::Settings(view::SettingsMessage::BitcoindSettings(
                msg,
            ))) => {
                if let Some(settings) = &mut self.bitcoind_settings {
                    return settings.update(daemon, cache, msg);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::ElectrumSettings(
                msg,
            ))) => {
                if let Some(settings) = &mut self.electrum_settings {
                    return settings.update(daemon, cache, msg);
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::RescanSettings(msg))) => {
                return self.rescan_settings.update(daemon, cache, msg);
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let can_edit_bitcoind_settings =
            self.bitcoind_settings.is_some() && !self.rescan_settings.processing;
        let can_edit_electrum_settings =
            self.electrum_settings.is_some() && !self.rescan_settings.processing;
        let settings_edit = self
            .bitcoind_settings
            .as_ref()
            .map(|settings| settings.edit)
            == Some(true)
            || self
                .electrum_settings
                .as_ref()
                .map(|settings| settings.edit)
                == Some(true);
        let can_do_rescan = !self.rescan_settings.processing && !settings_edit;
        view::settings::bitcoind_settings(
            cache,
            self.warning.as_ref(),
            if self.bitcoind_settings.is_some() || self.electrum_settings.is_some() {
                let mut setting_panels = Vec::new();
                if let Some(settings) = self.bitcoind_settings.as_ref() {
                    setting_panels.push(settings.view(cache, can_edit_bitcoind_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::BitcoindSettings(msg))
                        },
                    ))
                }
                if let Some(settings) = self.electrum_settings.as_ref() {
                    setting_panels.push(settings.view(cache, can_edit_electrum_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::ElectrumSettings(msg))
                        },
                    ))
                }
                setting_panels.push(self.rescan_settings.view(cache, can_do_rescan).map(
                    move |msg| view::Message::Settings(view::SettingsMessage::RescanSettings(msg)),
                ));
                setting_panels
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
    configured_node_type: Option<NodeType>,
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
        configured_node_type: Option<NodeType>,
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
        let addr = if configured_node_type == Some(NodeType::Bitcoind) {
            bitcoind_config.addr.to_string()
        } else {
            String::default()
        };
        BitcoindSettings {
            configured_node_type,
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
                        "socket_address" => self.addr.value = value,
                        "cookie_file_path" => self.rpc_auth_vals.cookie_path.value = value,
                        "user" => self.rpc_auth_vals.user.value = value,
                        "password" => self.rpc_auth_vals.password.value = value,
                        _ => {}
                    }
                }
            }
            view::SettingsEditMessage::ValidateDomainEdited(_) => {}
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
                        match new_path {
                            Ok(path) => {
                                self.rpc_auth_vals.cookie_path.valid = true;
                                Some(BitcoindRpcAuth::CookieFile(path))
                            }
                            Err(_) => None,
                        }
                    }
                    RpcAuthType::UserPass => Some(BitcoindRpcAuth::UserPass(
                        self.rpc_auth_vals.user.value.clone(),
                        self.rpc_auth_vals.password.value.clone(),
                    )),
                };

                if let (true, Some(rpc_auth)) = (self.addr.valid, rpc_auth) {
                    let mut daemon_config = daemon.config().cloned().unwrap();
                    daemon_config.bitcoin_backend =
                        Some(lianad::config::BitcoinBackend::Bitcoind(BitcoindConfig {
                            rpc_auth,
                            addr: new_addr.unwrap(),
                        }));
                    self.processing = true;
                    return Task::perform(async move { daemon_config }, |cfg| {
                        Message::LoadDaemonConfig(Box::new(cfg))
                    });
                }
            }
            view::SettingsEditMessage::Clipboard(text) => return clipboard::write(text),
        };
        Task::none()
    }

    fn view<'a>(&self, cache: &'a Cache, can_edit: bool) -> Element<'a, view::SettingsEditMessage> {
        let is_configured_node_type = self.configured_node_type == Some(NodeType::Bitcoind);
        if self.edit {
            view::settings::bitcoind_edit(
                is_configured_node_type,
                self.bitcoin_config.network,
                cache.blockheight,
                &self.addr,
                &self.rpc_auth_vals,
                &self.selected_auth_type,
                self.processing,
            )
        } else {
            view::settings::bitcoind(
                is_configured_node_type,
                self.bitcoin_config.network,
                &self.bitcoind_config,
                cache.blockheight,
                Some(cache.blockheight != 0),
                can_edit && !self.daemon_is_external && !self.bitcoind_is_internal,
            )
        }
    }
}

#[derive(Debug)]
pub struct ElectrumSettings {
    configured_node_type: Option<NodeType>,
    electrum_config: ElectrumConfig,
    bitcoin_config: BitcoinConfig,
    edit: bool,
    processing: bool,
    addr: form::Value<String>,
    daemon_is_external: bool,
}

impl ElectrumSettings {
    fn new(
        configured_node_type: Option<NodeType>,
        bitcoin_config: BitcoinConfig,
        electrum_config: ElectrumConfig,
        daemon_is_external: bool,
    ) -> ElectrumSettings {
        let addr = electrum_config.addr.to_string();
        ElectrumSettings {
            configured_node_type,
            daemon_is_external,
            electrum_config,
            bitcoin_config,
            edit: false,
            processing: false,
            addr: form::Value {
                valid: true,
                value: addr,
            },
        }
    }
}

impl ElectrumSettings {
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
                if !self.processing && field == "address" {
                    self.addr.valid = crate::node::electrum::is_electrum_address_valid(&value);
                    self.addr.value = value;
                }
            }
            view::SettingsEditMessage::Confirm => {
                if self.addr.valid {
                    let mut daemon_config = daemon.config().cloned().unwrap();
                    daemon_config.bitcoin_backend =
                        Some(lianad::config::BitcoinBackend::Electrum(ElectrumConfig {
                            addr: self.addr.value.clone(),
                            validate_domain: self.electrum_config.validate_domain,
                        }));
                    self.processing = true;
                    return Task::perform(async move { daemon_config }, |cfg| {
                        Message::LoadDaemonConfig(Box::new(cfg))
                    });
                }
            }
            view::SettingsEditMessage::Clipboard(text) => return clipboard::write(text),
            view::SettingsEditMessage::ValidateDomainEdited(b) => {
                if !self.processing {
                    self.electrum_config.validate_domain = b;
                }
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&self, cache: &'a Cache, can_edit: bool) -> Element<'a, view::SettingsEditMessage> {
        let is_configured_node_type = self.configured_node_type == Some(NodeType::Electrum);
        if self.edit {
            view::settings::electrum_edit(
                is_configured_node_type,
                self.bitcoin_config.network,
                cache.blockheight,
                &self.addr,
                self.processing,
                self.electrum_config.validate_domain,
            )
        } else {
            view::settings::electrum(
                is_configured_node_type,
                self.bitcoin_config.network,
                &self.electrum_config,
                cache.blockheight,
                Some(cache.blockheight != 0),
                can_edit && !self.daemon_is_external,
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
    future_date: bool,
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
        cache: &Cache,
        message: view::SettingsEditMessage,
    ) -> Task<Message> {
        match message {
            view::SettingsEditMessage::FieldEdited(field, value) => {
                self.invalid_date = false;
                self.future_date = false;
                self.past_possible_height = false;
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
                let t = if let Some(date) = NaiveDate::from_ymd_opt(
                    i32::from_str(&self.year.value).unwrap_or(1),
                    u32::from_str(&self.month.value).unwrap_or(1),
                    u32::from_str(&self.day.value).unwrap_or(1),
                )
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp())
                {
                    match cache.network {
                        Network::Bitcoin => {
                            if date < MAINNET_GENESIS_BLOCK_TIMESTAMP {
                                info!("Date {} prior to genesis block, using genesis block timestamp {}", date, MAINNET_GENESIS_BLOCK_TIMESTAMP);

                                MAINNET_GENESIS_BLOCK_TIMESTAMP
                            } else {
                                date
                            }
                        }
                        Network::Testnet => {
                            if date < TESTNET3_GENESIS_BLOCK_TIMESTAMP {
                                info!("Date {} prior to genesis block, using genesis block timestamp {}", date, TESTNET3_GENESIS_BLOCK_TIMESTAMP);
                                TESTNET3_GENESIS_BLOCK_TIMESTAMP
                            } else {
                                date
                            }
                        }
                        Network::Signet => {
                            if date < SIGNET_GENESIS_BLOCK_TIMESTAMP {
                                info!("Date {} prior to genesis block, using genesis block timestamp {}", date, SIGNET_GENESIS_BLOCK_TIMESTAMP);
                                SIGNET_GENESIS_BLOCK_TIMESTAMP
                            } else {
                                date
                            }
                        }
                        // We expect regtest user to not use genesis block timestamp inferior to
                        // the mainnet one.
                        // Network is a non exhaustive enum, that is why the _.
                        _ => {
                            if date < MAINNET_GENESIS_BLOCK_TIMESTAMP {
                                info!("Date {} prior to genesis block, using genesis block timestamp {}", date, MAINNET_GENESIS_BLOCK_TIMESTAMP);
                                MAINNET_GENESIS_BLOCK_TIMESTAMP
                            } else {
                                date
                            }
                        }
                    }
                } else {
                    self.invalid_date = true;
                    return Task::none();
                };
                if t > Utc::now().timestamp() {
                    self.future_date = true;
                    return Task::none();
                }
                self.processing = true;
                info!("Asking deamon to rescan with timestamp: {}", t);
                return Task::perform(
                    async move {
                        daemon.start_rescan(t.try_into().expect("t cannot be inferior to 0 otherwise genesis block timestamp is chosen"))
                            .await
                            .map_err(|e| e.into())
                    },
                    Message::StartRescan,
                );
            }
            _ => {}
        };
        Task::none()
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
            self.future_date,
        )
    }
}

/// Use bitcoin-cli getblock $(bitcoin-cli getblockhash 0) | jq .time
const MAINNET_GENESIS_BLOCK_TIMESTAMP: i64 = 1231006505;
const TESTNET3_GENESIS_BLOCK_TIMESTAMP: i64 = 1296688602;
const SIGNET_GENESIS_BLOCK_TIMESTAMP: i64 = 1598918400;
