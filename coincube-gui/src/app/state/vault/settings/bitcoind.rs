use std::convert::{From, TryInto};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use iced::{clipboard, Task};
use tracing::info;

use crate::services::coincube::{CoincubeClient, OtpRequest, OtpVerifyRequest};

use coincube_core::miniscript::bitcoin::Network;
use coincubed::config::{
    BitcoinBackend, BitcoinConfig, BitcoindConfig, BitcoindRpcAuth, Config, ElectrumConfig,
};

use coincube_ui::{component::form, icon, widget::Element};

use crate::{
    app::{
        cache::Cache, error::Error, menu::Menu, message::Message, state::vault::settings::State,
        view,
    },
    daemon::Daemon,
    dir::CoincubeDirectory,
    download,
    installer::step::node::bitcoind::{
        get_available_port, install_bitcoind, internal_bitcoind_address, PRUNE_DEFAULT,
    },
    node::{
        bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_cookie_path,
            internal_bitcoind_datadir, internal_bitcoind_directory, internal_bitcoind_exe_path,
            Bitcoind, InternalBitcoindConfig, InternalBitcoindConfigError,
            InternalBitcoindNetworkConfig, RpcAuthType, RpcAuthValues, VERSION,
        },
        NodeType,
    },
};

#[derive(Debug)]
enum ConnectLoginState {
    EnterEmail {
        client: CoincubeClient,
        email: String,
        loading: bool,
        error: Option<String>,
    },
    EnterOtp {
        client: CoincubeClient,
        email: String,
        otp: String,
        loading: bool,
        error: Option<String>,
    },
}

#[derive(Debug, PartialEq)]
enum InternalSetupStage {
    Idle,
    Downloading,
    Installing,
    Done,
}

#[derive(Debug)]
struct PendingNodeSetup {
    /// None = mode picker, Some(false) = self-managed external, Some(true) = COINCUBE-managed internal
    mode: Option<bool>,
    // External form fields
    addr: form::Value<String>,
    rpc_auth_vals: RpcAuthValues,
    selected_auth_type: RpcAuthType,
    processing: bool,
    // Internal (COINCUBE-managed) setup fields
    internal_stage: InternalSetupStage,
    internal_error: Option<String>,
    download_progress: f32,
}

#[derive(Debug)]
pub struct BitcoindSettingsState {
    warning: Option<Error>,
    config_updated: bool,
    full_config: Option<Config>,
    node_switch_processing: bool,
    connect_login: Option<ConnectLoginState>,
    pending_node_setup: Option<PendingNodeSetup>,
    cancel_node_setup_in_flight: bool,

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
            full_config: config.clone(),
            node_switch_processing: false,
            connect_login: None,
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
            rescan_settings: RescanSetting::new(cache.rescan_progress()),
            pending_node_setup: None,
            cancel_node_setup_in_flight: false,
        }
    }
}

impl State for BitcoindSettingsState {
    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("BitcoindSettingsState::update called without daemon");
            return Task::none();
        };
        match message {
            Message::DaemonConfigLoaded(res) => match res {
                Ok(()) => {
                    self.config_updated = true;
                    self.node_switch_processing = false;
                    self.connect_login = None;
                    self.pending_node_setup = None;
                    self.warning = None;
                    self.full_config = daemon.config().cloned();
                    if self.cancel_node_setup_in_flight {
                        self.cancel_node_setup_in_flight = false;
                        if let Some(cfg) = daemon.config() {
                            let mut rollback_cfg = cfg.clone();
                            rollback_cfg.pending_bitcoind = None;
                            return Task::done(Message::LoadDaemonConfig(Box::new(rollback_cfg)));
                        }
                    }
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
                    self.node_switch_processing = false;
                    self.connect_login = None;
                    self.pending_node_setup = None;
                    self.cancel_node_setup_in_flight = false;
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    if let Some(settings) = &mut self.bitcoind_settings {
                        settings.edited(false);
                    }
                    if let Some(settings) = &mut self.electrum_settings {
                        settings.edited(false);
                    }
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::Info(res) => match res {
                Err(e) => {
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
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
                self.rescan_settings.processing = cache.rescan_progress().is_some_and(|p| p < 1.0);
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
            Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(msg))) => {
                use view::NodeSettingsMessage;
                match msg {
                    NodeSettingsMessage::SwitchToConnect => {
                        // Gate the switch behind a fresh COINCUBE | Connect login
                        // to obtain a valid JWT before starting the daemon.
                        self.connect_login = Some(ConnectLoginState::EnterEmail {
                            client: CoincubeClient::new(),
                            email: String::new(),
                            loading: false,
                            error: None,
                        });
                    }
                    NodeSettingsMessage::ConnectLoginCancel => {
                        self.connect_login = None;
                    }
                    NodeSettingsMessage::ConnectLoginEmailChanged(email) => {
                        if let Some(ConnectLoginState::EnterEmail {
                            email: ref mut e, ..
                        }) = self.connect_login
                        {
                            *e = email;
                        }
                    }
                    NodeSettingsMessage::ConnectLoginRequestOtp => {
                        if let Some(ConnectLoginState::EnterEmail {
                            ref client,
                            ref email,
                            ref mut loading,
                            ref mut error,
                        }) = self.connect_login
                        {
                            if email.contains('@')
                                && email.contains('.')
                                && email.len() >= 6
                                && !*loading
                            {
                                *loading = true;
                                *error = None;
                                let client = client.clone();
                                let req = OtpRequest {
                                    email: email.clone(),
                                };
                                return Task::perform(
                                    async move {
                                        client.login_send_otp(req).await.map_err(|e| e.to_string())
                                    },
                                    |res| {
                                        Message::View(view::Message::Settings(
                                            view::SettingsMessage::NodeSettings(
                                                view::NodeSettingsMessage::ConnectLoginOtpRequested(
                                                    res,
                                                ),
                                            ),
                                        ))
                                    },
                                );
                            }
                        }
                    }
                    NodeSettingsMessage::ConnectLoginOtpRequested(res) => match res {
                        Ok(()) => {
                            let (client, email) = match self.connect_login.take() {
                                Some(ConnectLoginState::EnterEmail { client, email, .. }) => {
                                    (client, email)
                                }
                                other => {
                                    self.connect_login = other;
                                    return Task::none();
                                }
                            };
                            self.connect_login = Some(ConnectLoginState::EnterOtp {
                                client,
                                email,
                                otp: String::new(),
                                loading: false,
                                error: None,
                            });
                        }
                        Err(e) => {
                            if let Some(ConnectLoginState::EnterEmail {
                                ref mut loading,
                                ref mut error,
                                ..
                            }) = self.connect_login
                            {
                                *loading = false;
                                *error = Some(e);
                            }
                        }
                    },
                    NodeSettingsMessage::ConnectLoginOtpChanged(otp) => {
                        if let Some(ConnectLoginState::EnterOtp { otp: ref mut o, .. }) =
                            self.connect_login
                        {
                            *o = otp;
                        }
                    }
                    NodeSettingsMessage::ConnectLoginVerifyOtp => {
                        if let Some(ConnectLoginState::EnterOtp {
                            ref client,
                            ref email,
                            ref otp,
                            ref mut loading,
                            ref mut error,
                        }) = self.connect_login
                        {
                            if otp.len() == 6 && !*loading {
                                *loading = true;
                                *error = None;
                                let client = client.clone();
                                let req = OtpVerifyRequest {
                                    email: email.clone(),
                                    otp: otp.clone(),
                                };
                                return Task::perform(
                                    async move {
                                        client
                                            .login_verify_otp(req)
                                            .await
                                            .map(|resp| resp.token)
                                            .map_err(|e| e.to_string())
                                    },
                                    |res| {
                                        Message::View(view::Message::Settings(
                                            view::SettingsMessage::NodeSettings(
                                                view::NodeSettingsMessage::ConnectLoginVerified(
                                                    res,
                                                ),
                                            ),
                                        ))
                                    },
                                );
                            }
                        }
                    }
                    NodeSettingsMessage::ConnectLoginVerified(res) => match res {
                        Ok(jwt) => {
                            if matches!(
                                self.connect_login,
                                Some(ConnectLoginState::EnterOtp { .. })
                            ) {
                                self.connect_login = None;
                                if let Some(cfg) = daemon.config() {
                                    // Reconstruct URL from cache.network so a
                                    // stale fallback_esplora.addr (e.g. written
                                    // before Testnet4 was handled) is never used.
                                    use coincubed::config::EsploraConfig;
                                    let esplora_url = crate::installer::connect_url(cache.network);
                                    info!(
                                        "Switching to Connect: url={} token_len={}",
                                        esplora_url,
                                        jwt.len()
                                    );
                                    let esplora = EsploraConfig {
                                        addr: esplora_url,
                                        token: Some(jwt),
                                    };
                                    let mut new_cfg = cfg.clone();
                                    if let Some(BitcoinBackend::Bitcoind(current)) =
                                        cfg.bitcoin_backend.clone()
                                    {
                                        new_cfg.pending_bitcoind = Some(current);
                                    }
                                    new_cfg.bitcoin_backend =
                                        Some(BitcoinBackend::Esplora(esplora));
                                    new_cfg.fallback_esplora = None;
                                    self.node_switch_processing = true;
                                    self.warning = None;
                                    return Task::done(Message::LoadDaemonConfig(Box::new(
                                        new_cfg,
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            if let Some(ConnectLoginState::EnterOtp {
                                ref mut loading,
                                ref mut error,
                                ..
                            }) = self.connect_login
                            {
                                *loading = false;
                                *error = Some(e);
                            }
                        }
                    },
                    NodeSettingsMessage::SetupLocalNode => {
                        let default_addr = match cache.network {
                            Network::Bitcoin => "127.0.0.1:8332",
                            Network::Testnet => "127.0.0.1:18332",
                            Network::Signet => "127.0.0.1:38332",
                            Network::Testnet4 => "127.0.0.1:48332",
                            _ => "127.0.0.1:18443",
                        };
                        self.pending_node_setup = Some(PendingNodeSetup {
                            mode: None,
                            addr: form::Value {
                                valid: true,
                                warning: None,
                                value: default_addr.to_string(),
                            },
                            rpc_auth_vals: RpcAuthValues {
                                cookie_path: form::Value {
                                    valid: true,
                                    warning: None,
                                    value: String::new(),
                                },
                                user: form::Value::default(),
                                password: form::Value::default(),
                            },
                            selected_auth_type: RpcAuthType::CookieFile,
                            processing: false,
                            internal_stage: InternalSetupStage::Idle,
                            internal_error: None,
                            download_progress: 0.0,
                        });
                    }
                    NodeSettingsMessage::SetupLocalNodeModeSelected(use_internal) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            if use_internal {
                                setup.mode = Some(true);
                                setup.internal_error = None;
                                let coincube_datadir = cache.datadir_path.clone();
                                let network = cache.network;
                                let exe_exists =
                                    internal_bitcoind_exe_path(&coincube_datadir, VERSION).exists();
                                if exe_exists {
                                    setup.internal_stage = InternalSetupStage::Installing;
                                    return Task::perform(
                                        async move {
                                            tokio::task::spawn_blocking(move || {
                                                configure_and_start_internal_bitcoind(
                                                    coincube_datadir,
                                                    network,
                                                    None,
                                                )
                                            })
                                            .await
                                            .unwrap_or_else(|e| Err(e.to_string()))
                                        },
                                        |r| {
                                            Message::View(view::Message::Settings(
                                                view::SettingsMessage::NodeSettings(
                                                    view::NodeSettingsMessage::SetupLocalNodeStartResult(
                                                        r,
                                                    ),
                                                ),
                                            ))
                                        },
                                    );
                                } else {
                                    setup.internal_stage = InternalSetupStage::Downloading;
                                    let url = crate::node::bitcoind::download_url();
                                    return Task::sip(
                                        download::download(url),
                                        |p| {
                                            Message::View(view::Message::Settings(
                                                view::SettingsMessage::NodeSettings(
                                                    view::NodeSettingsMessage::SetupLocalNodeDownloadProgress(p.percent),
                                                ),
                                            ))
                                        },
                                        |r| {
                                            Message::View(view::Message::Settings(
                                                view::SettingsMessage::NodeSettings(
                                                    view::NodeSettingsMessage::SetupLocalNodeDownloadComplete(
                                                        r.map_err(|e| e.to_string()),
                                                    ),
                                                ),
                                            ))
                                        },
                                    );
                                }
                            } else {
                                setup.mode = Some(false);
                            }
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeDownloadProgress(p) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            setup.download_progress = p;
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeDownloadComplete(result) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            match result {
                                Ok(bytes) => {
                                    setup.internal_stage = InternalSetupStage::Installing;
                                    setup.download_progress = 100.0;
                                    let coincube_datadir = cache.datadir_path.clone();
                                    let network = cache.network;
                                    return Task::perform(
                                        async move {
                                            tokio::task::spawn_blocking(move || {
                                                configure_and_start_internal_bitcoind(
                                                    coincube_datadir,
                                                    network,
                                                    Some(bytes),
                                                )
                                            })
                                            .await
                                            .unwrap_or_else(|e| Err(e.to_string()))
                                        },
                                        |r| {
                                            Message::View(view::Message::Settings(
                                                view::SettingsMessage::NodeSettings(
                                                    view::NodeSettingsMessage::SetupLocalNodeStartResult(
                                                        r,
                                                    ),
                                                ),
                                            ))
                                        },
                                    );
                                }
                                Err(e) => {
                                    setup.internal_stage = InternalSetupStage::Downloading;
                                    setup.internal_error = Some(e);
                                }
                            }
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeStartResult(result) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            match result {
                                Ok((bitcoind_cfg, bitcoind)) => {
                                    if let Some(cfg) = daemon.config() {
                                        let mut new_cfg = cfg.clone();
                                        new_cfg.pending_bitcoind = Some(bitcoind_cfg);
                                        setup.internal_stage = InternalSetupStage::Done;
                                        setup.processing = true;
                                        return Task::batch([
                                            Task::done(Message::SetInternalBitcoind(bitcoind)),
                                            Task::done(Message::LoadDaemonConfig(Box::new(
                                                new_cfg,
                                            ))),
                                        ]);
                                    }
                                }
                                Err(e) => {
                                    setup.internal_error = Some(e);
                                }
                            }
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeCancel => {
                        if matches!(&self.pending_node_setup, Some(s) if s.processing) {
                            self.cancel_node_setup_in_flight = true;
                        }
                        self.pending_node_setup = None;
                    }
                    NodeSettingsMessage::SetupLocalNodeAddrChanged(addr) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            setup.addr.value = addr;
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeAuthTypeSelected(auth_type) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            setup.selected_auth_type = auth_type;
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeFieldEdited(field, value) => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            match field {
                                "cookie_file_path" => setup.rpc_auth_vals.cookie_path.value = value,
                                "user" => setup.rpc_auth_vals.user.value = value,
                                "password" => setup.rpc_auth_vals.password.value = value,
                                _ => {}
                            }
                        }
                    }
                    NodeSettingsMessage::SetupLocalNodeConfirm => {
                        if let Some(ref mut setup) = self.pending_node_setup {
                            if setup.processing {
                                return Task::none();
                            }
                            let new_addr = SocketAddr::from_str(&setup.addr.value);
                            setup.addr.valid = new_addr.is_ok();
                            let rpc_auth = match setup.selected_auth_type {
                                RpcAuthType::CookieFile => {
                                    if setup.rpc_auth_vals.cookie_path.value.is_empty() {
                                        setup.rpc_auth_vals.cookie_path.valid = false;
                                        None
                                    } else {
                                        let new_path = PathBuf::from_str(
                                            &setup.rpc_auth_vals.cookie_path.value,
                                        );
                                        match new_path {
                                            Ok(path) => {
                                                setup.rpc_auth_vals.cookie_path.valid = true;
                                                Some(BitcoindRpcAuth::CookieFile(path))
                                            }
                                            Err(_) => {
                                                setup.rpc_auth_vals.cookie_path.valid = false;
                                                None
                                            }
                                        }
                                    }
                                }
                                RpcAuthType::UserPass => {
                                    let user_ok = !setup.rpc_auth_vals.user.value.is_empty();
                                    let pass_ok = !setup.rpc_auth_vals.password.value.is_empty();
                                    setup.rpc_auth_vals.user.valid = user_ok;
                                    setup.rpc_auth_vals.password.valid = pass_ok;
                                    if user_ok && pass_ok {
                                        Some(BitcoindRpcAuth::UserPass(
                                            setup.rpc_auth_vals.user.value.clone(),
                                            setup.rpc_auth_vals.password.value.clone(),
                                        ))
                                    } else {
                                        None
                                    }
                                }
                            };
                            if let (Ok(addr), Some(rpc_auth)) = (new_addr, rpc_auth) {
                                if let Some(cfg) = daemon.config() {
                                    let mut new_cfg = cfg.clone();
                                    new_cfg.pending_bitcoind =
                                        Some(BitcoindConfig { rpc_auth, addr });
                                    setup.processing = true;
                                    return Task::done(Message::LoadDaemonConfig(Box::new(
                                        new_cfg,
                                    )));
                                }
                            }
                        }
                    }
                    NodeSettingsMessage::SwitchToBitcoind => {
                        match cache.node_bitcoind_ibd {
                            None => {
                                self.warning = Some(Error::Unexpected(
                                    "Bitcoin node sync status not yet known. \
                                     Please wait a moment and try again."
                                        .to_string(),
                                ));
                                return Task::none();
                            }
                            Some(true) => {
                                self.warning = Some(Error::Unexpected(format!(
                                    "Bitcoin node is still syncing ({:.1}%). \
                                     Please wait until sync is complete before switching.",
                                    cache.node_bitcoind_sync_progress.unwrap_or(0.0) * 100.0
                                )));
                                return Task::none();
                            }
                            Some(false) => {}
                        }
                        if let Some(cfg) = daemon.config() {
                            if let Some(pending) = cfg.pending_bitcoind.clone() {
                                let old_esplora = match &cfg.bitcoin_backend {
                                    Some(BitcoinBackend::Esplora(e)) => Some(e.clone()),
                                    _ => None,
                                };
                                let mut new_cfg = cfg.clone();
                                new_cfg.bitcoin_backend = Some(BitcoinBackend::Bitcoind(pending));
                                new_cfg.pending_bitcoind = None;
                                new_cfg.fallback_esplora = old_esplora;
                                self.node_switch_processing = true;
                                self.warning = None;
                                return Task::done(Message::LoadDaemonConfig(Box::new(new_cfg)));
                            }
                        }
                    }
                }
            }
            _ => {}
        };
        Task::none()
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
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
        view::vault::settings::bitcoind_settings(menu, cache, {
            let mut setting_panels = Vec::new();

            // Top panel: either Connect re-login flow or backend status + switch.
            let map_node_msg =
                |msg| view::Message::Settings(view::SettingsMessage::NodeSettings(msg));
            if let Some(ref setup) = self.pending_node_setup {
                match setup.mode {
                    None => {
                        setting_panels.push(
                            view::vault::settings::node_setup_mode_picker_panel().map(map_node_msg),
                        );
                    }
                    Some(false) => {
                        setting_panels.push(
                            view::vault::settings::pending_node_setup_panel(
                                &setup.addr,
                                &setup.rpc_auth_vals,
                                &setup.selected_auth_type,
                                setup.processing,
                            )
                            .map(map_node_msg),
                        );
                    }
                    Some(true) => {
                        setting_panels.push(
                            view::vault::settings::internal_node_setup_panel(
                                setup.internal_stage == InternalSetupStage::Downloading,
                                setup.internal_stage == InternalSetupStage::Installing,
                                setup.internal_stage == InternalSetupStage::Done,
                                setup.internal_error.as_deref(),
                                setup.download_progress,
                            )
                            .map(map_node_msg),
                        );
                    }
                }
            } else if let Some(login) = &self.connect_login {
                let (step, email, otp, loading, error) = match login {
                    ConnectLoginState::EnterEmail {
                        email,
                        loading,
                        error,
                        ..
                    } => (
                        view::vault::settings::ConnectLoginViewStep::EnterEmail,
                        email.as_str(),
                        "",
                        *loading,
                        error.clone(),
                    ),
                    ConnectLoginState::EnterOtp {
                        email,
                        otp,
                        loading,
                        error,
                        ..
                    } => (
                        view::vault::settings::ConnectLoginViewStep::EnterOtp,
                        email.as_str(),
                        otp.as_str(),
                        *loading,
                        error.clone(),
                    ),
                };
                setting_panels.push(
                    view::vault::settings::connect_login_panel(&step, email, otp, loading, error)
                        .map(map_node_msg),
                );
            } else {
                let (
                    active_backend,
                    active_icon,
                    can_switch_to_connect,
                    can_switch_to_bitcoind,
                    can_setup_local_node,
                ) = if let Some(cfg) = &self.full_config {
                    let (ab, ai) = match &cfg.bitcoin_backend {
                        Some(BitcoinBackend::Esplora(_)) => {
                            ("COINCUBE | Connect", icon::network_icon())
                        }
                        Some(BitcoinBackend::Bitcoind(_)) => {
                            ("Local Node (Bitcoin Core)", icon::bitcoin_icon())
                        }
                        Some(BitcoinBackend::Electrum(_)) => ("Electrum", icon::network_icon()),
                        None => ("None", icon::network_icon()),
                    };
                    let ctc = cfg.fallback_esplora.is_some()
                        && matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_)));
                    let ctb = cfg.pending_bitcoind.is_some()
                        && matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Esplora(_)));
                    let csl = matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Esplora(_)))
                        && cfg.pending_bitcoind.is_none();
                    (ab, ai, ctc, ctb, csl)
                } else {
                    ("None", icon::network_icon(), false, false, false)
                };
                let warning_str = self
                    .warning
                    .as_ref()
                    .filter(|_| {
                        self.node_switch_processing
                            || can_switch_to_connect
                            || can_switch_to_bitcoind
                    })
                    .map(|e| e.to_string());
                setting_panels.push(
                    view::vault::settings::node_backend_status(
                        active_backend,
                        active_icon,
                        cache.node_bitcoind_sync_progress,
                        cache.node_bitcoind_last_log.as_deref(),
                        can_switch_to_connect,
                        can_switch_to_bitcoind,
                        can_setup_local_node,
                        self.node_switch_processing,
                        warning_str,
                    )
                    .map(map_node_msg),
                );
            }

            if self.bitcoind_settings.is_some() || self.electrum_settings.is_some() {
                if let Some(settings) = self.bitcoind_settings.as_ref() {
                    setting_panels.push(settings.view(cache, can_edit_bitcoind_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::BitcoindSettings(msg))
                        },
                    ));
                }
                if let Some(settings) = self.electrum_settings.as_ref() {
                    setting_panels.push(settings.view(cache, can_edit_electrum_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::ElectrumSettings(msg))
                        },
                    ));
                }
            }
            setting_panels.push(
                self.rescan_settings
                    .view(cache, can_do_rescan)
                    .map(move |msg| {
                        view::Message::Settings(view::SettingsMessage::RescanSettings(msg))
                    }),
            );
            setting_panels
        })
    }
}

impl From<BitcoindSettingsState> for Box<dyn State> {
    fn from(s: BitcoindSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

/// Configure and start an internally-managed pruned Bitcoin Core node.
/// If `bytes_to_install` is Some, the binary is first installed from those bytes.
/// Returns the `BitcoindConfig` and the live `Bitcoind` handle (which keeps the
/// lock file alive) to be stored by the caller.
fn configure_and_start_internal_bitcoind(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    bytes_to_install: Option<Vec<u8>>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    if let Some(ref bytes) = bytes_to_install {
        let install_dir = internal_bitcoind_directory(&coincube_datadir);
        install_bitcoind(&install_dir, bytes).map_err(|e| format!("{:?}", e))?;
    }

    let bitcoind_datadir = internal_bitcoind_datadir(&coincube_datadir);
    let config_path = internal_bitcoind_config_path(&bitcoind_datadir);

    let mut conf = match InternalBitcoindConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(InternalBitcoindConfigError::FileNotFound) => InternalBitcoindConfig::new(),
        Err(e) => return Err(e.to_string()),
    };

    let existing = conf.networks.get(&network).cloned();
    let (rpc_port, p2p_port) = if let Some(ref nc) = existing {
        (nc.rpc_port, nc.p2p_port)
    } else {
        let rpc = get_available_port().map_err(|e: crate::installer::Error| e.to_string())?;
        let p2p = get_available_port().map_err(|e: crate::installer::Error| e.to_string())?;
        if rpc == p2p {
            return Err("Could not get distinct ports. Please try again.".to_string());
        }
        (rpc, p2p)
    };

    let network_conf = existing.unwrap_or(InternalBitcoindNetworkConfig {
        rpc_port,
        p2p_port,
        prune: PRUNE_DEFAULT,
        rpc_auth: None,
    });
    conf.networks.insert(network, network_conf);
    conf.to_file(&config_path).map_err(|e| e.to_string())?;

    let cookie_path = internal_bitcoind_cookie_path(&bitcoind_datadir, &network);
    let bitcoind_config = BitcoindConfig {
        rpc_auth: BitcoindRpcAuth::CookieFile(cookie_path),
        addr: internal_bitcoind_address(rpc_port),
    };

    let bitcoind = Bitcoind::maybe_start(network, bitcoind_config.clone(), &coincube_datadir)
        .map_err(|e| e.to_string())?;

    Ok((bitcoind_config, bitcoind))
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
                        warning: None,
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
                        warning: None,
                        value: user.clone(),
                    },
                    password: form::Value {
                        valid: true,
                        warning: None,
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
                warning: None,
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
                    daemon_config.bitcoin_backend = Some(
                        coincubed::config::BitcoinBackend::Bitcoind(BitcoindConfig {
                            rpc_auth,
                            addr: new_addr.unwrap(),
                        }),
                    );
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
            view::vault::settings::bitcoind_edit(
                is_configured_node_type,
                self.bitcoin_config.network,
                cache.blockheight(),
                &self.addr,
                &self.rpc_auth_vals,
                &self.selected_auth_type,
                self.processing,
            )
        } else {
            view::vault::settings::bitcoind(
                is_configured_node_type,
                self.bitcoin_config.network,
                &self.bitcoind_config,
                cache.blockheight(),
                Some(cache.blockheight() != 0),
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
                warning: None,
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
                    daemon_config.bitcoin_backend = Some(
                        coincubed::config::BitcoinBackend::Electrum(ElectrumConfig {
                            addr: self.addr.value.clone(),
                            validate_domain: self.electrum_config.validate_domain,
                        }),
                    );
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
            view::vault::settings::electrum_edit(
                is_configured_node_type,
                self.bitcoin_config.network,
                cache.blockheight(),
                &self.addr,
                self.processing,
                self.electrum_config.validate_domain,
            )
        } else {
            view::vault::settings::electrum(
                is_configured_node_type,
                self.bitcoin_config.network,
                &self.electrum_config,
                cache.blockheight(),
                Some(cache.blockheight() != 0),
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
                        Network::Testnet4 => {
                            if date < TESTNET4_GENESIS_BLOCK_TIMESTAMP {
                                info!("Date {} prior to genesis block, using genesis block timestamp {}", date, TESTNET4_GENESIS_BLOCK_TIMESTAMP);
                                TESTNET4_GENESIS_BLOCK_TIMESTAMP
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
                info!("Asking daemon to rescan with timestamp: {}", t);
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
        view::vault::settings::rescan(
            &self.year,
            &self.month,
            &self.day,
            cache.rescan_progress(),
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
const TESTNET4_GENESIS_BLOCK_TIMESTAMP: i64 = 1714777860;
const SIGNET_GENESIS_BLOCK_TIMESTAMP: i64 = 1598918400;
