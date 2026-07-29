use std::convert::{From, TryInto};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use iced::{clipboard, Task};
use tracing::{info, warn};

use coincube_core::miniscript::bitcoin::Network;
use coincubed::config::{
    BitcoinBackend, BitcoinConfig, BitcoindConfig, BitcoindRpcAuth, Config, ElectrumConfig,
};

use coincube_ui::{
    component::form,
    icon,
    widget::{modal, Element},
};

use crate::{
    app::{
        cache::Cache, error::Error, menu::Menu, message::Message, state::vault::settings::State,
        view,
    },
    daemon::Daemon,
    dir::CoincubeDirectory,
    download,
    installer::step::node::bitcoind::{
        get_available_port, install_bitcoind, internal_bitcoind_address, DownloadVerification,
        PRUNE_DEFAULT,
    },
    node::{
        bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_cookie_path,
            internal_bitcoind_datadir, internal_bitcoind_directory, internal_bitcoind_exe_path,
            Bitcoind, InternalBitcoindConfig, InternalBitcoindConfigError,
            InternalBitcoindNetworkConfig, NodeFlavor, NodeResources, RpcAuthType, RpcAuthValues,
        },
        NodeType,
    },
};

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
    /// Which managed node flavour to install (Core or Knots + RDTS).
    flavor: NodeFlavor,
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
    pending_node_setup: Option<PendingNodeSetup>,
    cancel_node_setup_in_flight: bool,

    bitcoind_settings: Option<BitcoindSettings>,
    electrum_settings: Option<ElectrumSettings>,
    rescan_settings: RescanSetting,
    /// Cached inbound-over-Tor preference (from the sidecar), driving the "Help
    /// defend the network" toggles. Persisted on every change.
    inbound_tor_pref: crate::node::tor::InboundTorPreference,
    /// A managed-node flavour the user picked from the node-card dropdown but
    /// hasn't confirmed yet — while `Some`, a confirmation panel is shown.
    pending_flavor_switch: Option<NodeFlavor>,
    /// Node-resources editor for the internal managed node: prune target (MB) and
    /// mempool cap (MB, blank = bitcoind's 300 MB default). Loaded from the
    /// on-disk `bitcoin.conf` for the active network; applied via a force-restart.
    node_prune_mb: form::Value<String>,
    node_max_mempool_mb: form::Value<String>,
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
        // Pre-fill the node-resources editor from the on-disk managed config for
        // the active network, so the fields reflect reality. Absent config (e.g.
        // an external/Connect backend) falls back to the defaults; the section is
        // only shown for the internal managed node anyway.
        let managed_conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(
            &internal_bitcoind_datadir(&cache.datadir_path),
        ))
        .ok();
        let managed_prune_mb = managed_conf
            .as_ref()
            .and_then(|c| c.networks.get(&cache.network))
            .map(|n| n.prune)
            .unwrap_or(PRUNE_DEFAULT);
        let managed_max_mempool_mb = managed_conf
            .as_ref()
            .and_then(|c| c.max_mempool_mb)
            .map(|mb| mb.to_string())
            .unwrap_or_default();
        BitcoindSettingsState {
            warning: None,
            config_updated: false,
            full_config: config.clone(),
            node_switch_processing: false,
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
            inbound_tor_pref: crate::node::tor::InboundTorPreference::load(&cache.datadir_path),
            pending_flavor_switch: None,
            node_prune_mb: form::Value {
                value: managed_prune_mb.to_string(),
                valid: true,
                warning: None,
            },
            node_max_mempool_mb: form::Value {
                value: managed_max_mempool_mb,
                valid: true,
                warning: None,
            },
        }
    }

    /// Persist the current inbound-over-Tor preference to its sidecar.
    fn persist_inbound_tor_pref(&self, datadir: &CoincubeDirectory) {
        if let Err(e) = self.inbound_tor_pref.save(datadir) {
            warn!("could not save inbound-tor preference: {e}");
        }
    }

    /// Apply a one-click machine-profile preset (Small/Regular computer) to the
    /// node-resources editor fields. Nothing is written until the user hits
    /// "Restart node to apply".
    fn apply_node_resource_preset(&mut self, r: NodeResources) {
        crate::node::bitcoind::set_prune_form_value(
            &mut self.node_prune_mb,
            r.prune_mb.to_string(),
        );
        crate::node::bitcoind::set_max_mempool_form_value(
            &mut self.node_max_mempool_mb,
            r.max_mempool_mb
                .map(|mb| mb.to_string())
                .unwrap_or_default(),
        );
    }

    /// Build an `EsploraConfig` carrying `jwt` and dispatch a `LoadDaemonConfig`
    /// task that flips the active backend to Connect. Shared by the post-OTP
    /// path and the App-level fast path that reuses an existing Connect
    /// session.
    fn apply_connect_jwt(
        &mut self,
        daemon: &Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        jwt: String,
    ) -> Task<Message> {
        let Some(cfg) = daemon.config() else {
            warn!(
                "apply_connect_jwt: daemon.config() is None \
                 (external coincubed?) — cannot switch to Connect"
            );
            let err = Error::Unexpected("Cannot enable Connect: configuration missing".to_string());
            let err_msg = err.to_string();
            self.warning = Some(err);
            return Task::done(Message::View(view::Message::ShowError(err_msg)));
        };
        // Reconstruct the provider chain from cache.network so a stale
        // fallback_esplora.addr (e.g. written before Testnet4 was handled) is
        // never used. Mainnet routes primary traffic through COINCUBE API
        // (keeps addresses off public providers); every other network keeps a
        // public-primary chain (mempool.space → blockstream.info → Connect).
        // See `connect_esplora_config` for the assembled chain.
        let esplora = crate::installer::connect_esplora_config(cache.network, &jwt);
        info!(
            "Switching to Connect: primary={} fallback={:?} secondary_fallback={:?} token_len={}",
            esplora.addr,
            esplora.fallback_addr,
            esplora.secondary_fallback_addr,
            jwt.len()
        );
        let mut new_cfg = cfg.clone();
        if let Some(BitcoinBackend::Bitcoind(current)) = cfg.bitcoin_backend.clone() {
            new_cfg.pending_bitcoind = Some(current);
        }
        // The user deliberately switched to Connect — park the node, but don't
        // auto-revert to it on the next sync probe.
        new_cfg.auto_switch_to_pending = Some(false);
        new_cfg.bitcoin_backend = Some(BitcoinBackend::Esplora(esplora));
        // Bump the poll cadence to the Esplora-safe interval so
        // we don't carry a snappy localhost cadence into a path
        // that pays HTTP cost per poll and would burn through
        // public-tier rate windows. See
        // `coincubed::config::ESPLORA_POLL_INTERVAL_SECS`.
        new_cfg.bitcoin_config.poll_interval_secs =
            std::time::Duration::from_secs(coincubed::config::ESPLORA_POLL_INTERVAL_SECS);
        new_cfg.fallback_esplora = None;
        self.node_switch_processing = true;
        self.warning = None;
        Task::done(Message::LoadDaemonConfig(Box::new(new_cfg)))
    }

    /// Begin (or retry) the COINCUBE-managed local-node setup for the flavour
    /// stored on `pending_node_setup`. Reuses an installed binary if present,
    /// otherwise kicks off the download. Shared by the managed-flavour picker
    /// and the retry button.
    fn start_internal_node_setup(&mut self, cache: &Cache) -> Task<Message> {
        let Some(setup) = self.pending_node_setup.as_mut() else {
            return Task::none();
        };
        setup.mode = Some(true);
        setup.internal_error = None;
        let flavor = setup.flavor;
        let coincube_datadir = cache.datadir_path.clone();
        let network = cache.network;
        let exe_exists = internal_bitcoind_exe_path(&coincube_datadir, flavor.version()).exists();
        if exe_exists {
            setup.internal_stage = InternalSetupStage::Installing;
            Task::perform(
                ensure_tor_and_start_managed(coincube_datadir, network, flavor, None, false, None),
                |r| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::NodeSettings(
                            view::NodeSettingsMessage::SetupLocalNodeStartResult(r),
                        ),
                    ))
                },
            )
        } else {
            setup.internal_stage = InternalSetupStage::Downloading;
            let url = flavor.download_url();
            Task::sip(
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
            )
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
                    self.pending_node_setup = None;
                    self.warning = None;
                    self.full_config = daemon.config().cloned();
                    if self.cancel_node_setup_in_flight {
                        self.cancel_node_setup_in_flight = false;
                        if let Some(cfg) = daemon.config() {
                            let mut rollback_cfg = cfg.clone();
                            rollback_cfg.pending_bitcoind = None;
                            // Keep the invariant: no pending node ⇒ nothing to
                            // auto-switch to. Leaving the flag set would strand a
                            // stale "auto-switch enabled" state with no target.
                            rollback_cfg.auto_switch_to_pending = Some(false);
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
                // Intercept the node-card flavour dropdown before delegating to
                // the sub-state: raise a confirmation instead of switching
                // immediately (a switch restarts the shared node). Match by
                // reference so `msg` stays available for delegation.
                if let view::SettingsEditMessage::SwitchManagedFlavor(flavor) = &msg {
                    let flavor = *flavor;
                    let current = self
                        .bitcoind_settings
                        .as_ref()
                        .and_then(|s| s.managed_flavor);
                    if current != Some(flavor) {
                        self.pending_flavor_switch = Some(flavor);
                    }
                    return Task::none();
                }
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
                        // The App-level dispatcher always rewrites this into
                        // either `SwitchToConnectFastPath(jwt)` (if a Connect
                        // session is already live) or a navigation to the
                        // Connect tab to sign in. We never land here directly.
                    }
                    NodeSettingsMessage::SwitchToConnectFastPath(jwt) => {
                        return self.apply_connect_jwt(&daemon, cache, jwt.into_string());
                    }
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
                            // The managed node is shared by every Vault, so its
                            // flavour is global: default the picker to whatever is
                            // already configured, else Knots (matching the
                            // installer). Switching it restarts the node for all
                            // Vaults (handled by `maybe_start`).
                            flavor: InternalBitcoindConfig::from_file(
                                &internal_bitcoind_config_path(&internal_bitcoind_datadir(
                                    &cache.datadir_path,
                                )),
                            )
                            .map(|c| c.flavor)
                            .unwrap_or(NodeFlavor::Knots),
                            internal_stage: InternalSetupStage::Idle,
                            internal_error: None,
                            download_progress: 0.0,
                        });
                    }
                    NodeSettingsMessage::SetupLocalNodeManagedFlavor(flavor) => {
                        // Pick the managed flavour (Core or Knots + RDTS) and
                        // begin the download/install in one step. This is where
                        // the user consents to RDTS enforcement: the headless
                        // node can't show Knots' own confirmation prompt.
                        if let Some(ref mut setup) = self.pending_node_setup {
                            setup.flavor = flavor;
                        }
                        return self.start_internal_node_setup(cache);
                    }
                    NodeSettingsMessage::SetupLocalNodeModeSelected(use_internal) => {
                        if use_internal {
                            // Retry path: re-run with the already-chosen flavour.
                            return self.start_internal_node_setup(cache);
                        } else if let Some(ref mut setup) = self.pending_node_setup {
                            setup.mode = Some(false);
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
                                    let flavor = setup.flavor;
                                    return Task::perform(
                                        async move {
                                            // Fetch the release SHA256SUMS(+.asc)
                                            // the archive is verified against
                                            // (Knots); a no-op for Core.
                                            let manifest = download::fetch_release_manifest(flavor)
                                                .await
                                                .map_err(|e| e.to_string())?;
                                            ensure_tor_and_start_managed(
                                                coincube_datadir,
                                                network,
                                                flavor,
                                                Some((bytes, manifest)),
                                                false,
                                                None,
                                            )
                                            .await
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
                                        // Adopted a freshly set-up node — switch to
                                        // it once synced.
                                        new_cfg.auto_switch_to_pending = Some(true);
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
                                    // Adopted an existing/external node — switch to
                                    // it once it's reachable and synced.
                                    new_cfg.auto_switch_to_pending = Some(true);
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
                                // The pending node is now the active backend, so
                                // clear the auto-switch flag too — keeping it set
                                // with no pending target violates the invariant.
                                new_cfg.auto_switch_to_pending = Some(false);
                                new_cfg.fallback_esplora = old_esplora;
                                // Drop the poll cadence back to the
                                // snappy local-node interval. The
                                // Esplora-safe 10-min value made
                                // sense for HTTPS-per-poll against
                                // a rate-limited public provider;
                                // bitcoind is a free localhost RPC.
                                new_cfg.bitcoin_config.poll_interval_secs =
                                    std::time::Duration::from_secs(
                                        coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS,
                                    );
                                self.node_switch_processing = true;
                                self.warning = None;
                                return Task::done(Message::LoadDaemonConfig(Box::new(new_cfg)));
                            }
                        }
                    }
                    // "Help defend the network": persist the preference sidecar.
                    // The change takes effect the next time the managed node
                    // starts (see `node::tor::prepare_inbound_tor`), so there's
                    // nothing to restart here.
                    NodeSettingsMessage::InboundTorToggled(on) => {
                        self.inbound_tor_pref.enabled = on;
                        self.persist_inbound_tor_pref(&cache.datadir_path);
                    }
                    NodeSettingsMessage::InboundTorOutboundToggled(on) => {
                        self.inbound_tor_pref.outbound_via_tor = on;
                        self.persist_inbound_tor_pref(&cache.datadir_path);
                    }
                    NodeSettingsMessage::InboundTorLimitUploadToggled(limit) => {
                        self.inbound_tor_pref.max_upload_target_mb_day = if limit {
                            Some(crate::node::bitcoind::MAX_UPLOAD_TARGET_MB_DAY_DEFAULT)
                        } else {
                            None
                        };
                        self.persist_inbound_tor_pref(&cache.datadir_path);
                    }
                    NodeSettingsMessage::ConfirmFlavorSwitch => {
                        if let Some(flavor) = self.pending_flavor_switch.take() {
                            // Reuse the managed-node setup flow: it downloads the
                            // binary if needed, rewrites the conf, and restarts on
                            // the new flavour (`maybe_start` stops the old one).
                            self.pending_node_setup = Some(PendingNodeSetup {
                                mode: Some(true),
                                addr: form::Value::default(),
                                rpc_auth_vals: RpcAuthValues::default(),
                                selected_auth_type: RpcAuthType::CookieFile,
                                processing: false,
                                flavor,
                                internal_stage: InternalSetupStage::Idle,
                                internal_error: None,
                                download_progress: 0.0,
                            });
                            return self.start_internal_node_setup(cache);
                        }
                    }
                    NodeSettingsMessage::CancelFlavorSwitch => {
                        self.pending_flavor_switch = None;
                    }
                    NodeSettingsMessage::CopyToClipboard(value) => {
                        return clipboard::write(value);
                    }
                    NodeSettingsMessage::RepairNodeChain => {
                        // Manual counterpart to the automatic check in
                        // `Bitcoind::maybe_start`. Idempotent and non-destructive:
                        // `reconsiderblock` only clears rejection flags and lets the
                        // node re-activate the most-work chain, so the worst case is
                        // that it does nothing.
                        let Some(settings) = self.bitcoind_settings.as_ref() else {
                            return Task::none();
                        };
                        let cfg = settings.bitcoind_config.clone();
                        let network = cache.network;
                        let coincube_datadir = cache.datadir_path.clone();
                        return Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    repair_managed_node_chain(&coincube_datadir, &cfg, network)
                                })
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()))
                            },
                            |res| match res {
                                Ok(msg) => {
                                    Message::View(view::Message::ShowToast(log::Level::Info, msg))
                                }
                                Err(e) => Message::View(view::Message::ShowError(e)),
                            },
                        );
                    }
                    NodeSettingsMessage::RestartNodeToApply => {
                        // Restart the managed node now so freshly-toggled
                        // inbound-over-Tor settings take effect, reusing the setup
                        // progress panel + result handler. `force_restart` stops
                        // the running node first (same flavour, so `maybe_start`
                        // would otherwise reuse it).
                        let Some(flavor) = self
                            .bitcoind_settings
                            .as_ref()
                            .and_then(|s| s.managed_flavor)
                        else {
                            return Task::none();
                        };
                        self.pending_node_setup = Some(PendingNodeSetup {
                            mode: Some(true),
                            addr: form::Value::default(),
                            rpc_auth_vals: RpcAuthValues::default(),
                            selected_auth_type: RpcAuthType::CookieFile,
                            processing: true,
                            flavor,
                            internal_stage: InternalSetupStage::Installing,
                            internal_error: None,
                            download_progress: 100.0,
                        });
                        let coincube_datadir = cache.datadir_path.clone();
                        let network = cache.network;
                        return Task::perform(
                            ensure_tor_and_start_managed(
                                coincube_datadir,
                                network,
                                flavor,
                                None,
                                true,
                                None,
                            ),
                            |r| {
                                Message::View(view::Message::Settings(
                                    view::SettingsMessage::NodeSettings(
                                        view::NodeSettingsMessage::SetupLocalNodeStartResult(r),
                                    ),
                                ))
                            },
                        );
                    }
                    NodeSettingsMessage::NodeResourcePruneEdited(value) => {
                        crate::node::bitcoind::set_prune_form_value(&mut self.node_prune_mb, value);
                    }
                    NodeSettingsMessage::NodeResourceMaxMempoolEdited(value) => {
                        crate::node::bitcoind::set_max_mempool_form_value(
                            &mut self.node_max_mempool_mb,
                            value,
                        );
                    }
                    NodeSettingsMessage::NodeResourceSmallComputer => {
                        self.apply_node_resource_preset(NodeResources::small_computer());
                    }
                    NodeSettingsMessage::NodeResourceRegularComputer => {
                        self.apply_node_resource_preset(NodeResources::regular_computer());
                    }
                    NodeSettingsMessage::NodeResourceApply => {
                        // Validate the fields; bail (leaving them flagged) on bad
                        // input so nothing below bitcoind's floors reaches disk.
                        let Some(resources) = crate::node::bitcoind::resources_from_forms(
                            &mut self.node_prune_mb,
                            &mut self.node_max_mempool_mb,
                        ) else {
                            return Task::none();
                        };
                        // Only the internal managed node carries resource settings.
                        let Some(flavor) = self
                            .bitcoind_settings
                            .as_ref()
                            .and_then(|s| s.managed_flavor)
                        else {
                            return Task::none();
                        };
                        // Reuse the setup progress panel + result handler; the
                        // force-restart applies the rewritten conf (same flavour,
                        // so `maybe_start` would otherwise reuse the running node).
                        self.pending_node_setup = Some(PendingNodeSetup {
                            mode: Some(true),
                            addr: form::Value::default(),
                            rpc_auth_vals: RpcAuthValues::default(),
                            selected_auth_type: RpcAuthType::CookieFile,
                            processing: true,
                            flavor,
                            internal_stage: InternalSetupStage::Installing,
                            internal_error: None,
                            download_progress: 100.0,
                        });
                        let coincube_datadir = cache.datadir_path.clone();
                        let network = cache.network;
                        return Task::perform(
                            ensure_tor_and_start_managed(
                                coincube_datadir,
                                network,
                                flavor,
                                None,
                                true,
                                Some(resources),
                            ),
                            |r| {
                                Message::View(view::Message::Settings(
                                    view::SettingsMessage::NodeSettings(
                                        view::NodeSettingsMessage::SetupLocalNodeStartResult(r),
                                    ),
                                ))
                            },
                        );
                    }
                }
            }
            _ => {}
        }
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
        let content = view::vault::settings::bitcoind_settings(menu, cache, {
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
                                setup.flavor,
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
                            // Flavour-neutral: the exact build (Core vs Knots)
                            // and RDTS status come from the node's runtime
                            // subversion, surfaced separately below.
                            ("Local Node", icon::bitcoin_icon())
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
                    // When the config exists but no backend is configured,
                    // unlock the "Use Connect" / "Set up local node" paths
                    // so the user has a recovery route. Without this the
                    // page renders read-only "None" with no escape hatch.
                    let (ctc, csl) = if cfg.bitcoin_backend.is_none() {
                        warn!(
                            "Node settings: daemon config has no bitcoin_backend; \
                             unlocking recovery buttons. fallback_esplora={:?} pending_bitcoind={:?}",
                            cfg.fallback_esplora.is_some(),
                            cfg.pending_bitcoind.is_some(),
                        );
                        (true, true)
                    } else {
                        (ctc, csl)
                    };
                    (ab, ai, ctc, ctb, csl)
                } else {
                    // No daemon config at all (e.g. ExternalCoincubed). The
                    // GUI has nothing to write here, so leave the recovery
                    // buttons hidden; clicking them would silently fail
                    // because `apply_connect_jwt` early-returns on missing
                    // config. Logged so we notice if this state appears
                    // for setups where the GUI *should* own the config.
                    warn!(
                        "Node settings: daemon.config() returned None \
                         (external coincubed?) — backend picker hidden"
                    );
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
                        cache.node_bitcoind_ibd,
                        cache.node_bitcoind_last_log.as_deref(),
                        can_switch_to_connect,
                        can_switch_to_bitcoind,
                        can_setup_local_node,
                        self.node_switch_processing,
                        cache.daemon_switch_in_progress,
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

                // "Help defend the network": inbound-over-Tor controls, placed
                // directly below the node card. Shown only for the managed local
                // node on mainnet (the feature is mainnet-only), and hidden
                // during a node setup/flavour switch.
                if cache.network == Network::Bitcoin
                    && self.pending_node_setup.is_none()
                    && matches!(
                        self.full_config
                            .as_ref()
                            .and_then(|c| c.bitcoin_backend.as_ref()),
                        Some(BitcoinBackend::Bitcoind(_))
                    )
                {
                    setting_panels.push(
                        view::vault::settings::inbound_tor_section(
                            self.inbound_tor_pref.enabled,
                            self.inbound_tor_pref.outbound_via_tor,
                            self.inbound_tor_pref.max_upload_target_mb_day.is_some(),
                            crate::node::tor::managed_tor_ports().is_some(),
                            crate::node::tor::onion_key_exists(&cache.datadir_path, cache.network),
                            crate::node::bitcoind::tor_supported_on_host(),
                            cache.node_net_stats.as_ref(),
                        )
                        .map(map_node_msg),
                    );
                }

                // "Chain repair": manual `reconsiderblock` at the BIP-110 anchor.
                // Managed node, mainnet only (that's where RDTS is deployed), and
                // hidden during a setup/flavour switch. The automatic check on node
                // start covers the normal case; this is the escape hatch for when
                // the state that drives it has been lost.
                if crate::node::revalidate::rdts_anchor_height(cache.network).is_some()
                    && self.pending_node_setup.is_none()
                    && self
                        .bitcoind_settings
                        .as_ref()
                        .and_then(|s| s.managed_flavor)
                        .is_some()
                    && matches!(
                        self.full_config
                            .as_ref()
                            .and_then(|c| c.bitcoin_backend.as_ref()),
                        Some(BitcoinBackend::Bitcoind(_))
                    )
                {
                    setting_panels
                        .push(view::vault::settings::chain_repair_section().map(map_node_msg));
                }

                // "Node resources": prune target + mempool cap for the internal
                // managed node. All networks (unlike inbound-Tor), the managed
                // node only, and hidden during a setup/flavour switch/restart.
                if self.pending_node_setup.is_none()
                    && self
                        .bitcoind_settings
                        .as_ref()
                        .and_then(|s| s.managed_flavor)
                        .is_some()
                    && matches!(
                        self.full_config
                            .as_ref()
                            .and_then(|c| c.bitcoin_backend.as_ref()),
                        Some(BitcoinBackend::Bitcoind(_))
                    )
                {
                    setting_panels.push(
                        view::vault::settings::node_resources_section(
                            &self.node_prune_mb,
                            &self.node_max_mempool_mb,
                            false,
                        )
                        .map(map_node_msg),
                    );
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
        });

        // A pending Core↔Knots switch pops a confirmation modal over the page;
        // clicking the backdrop cancels it.
        if let Some(flavor) = self.pending_flavor_switch {
            modal::Modal::new(
                content,
                view::vault::settings::flavor_switch_confirm(flavor)
                    .map(|m| view::Message::Settings(view::SettingsMessage::NodeSettings(m))),
            )
            .on_blur(Some(view::Message::Settings(
                view::SettingsMessage::NodeSettings(view::NodeSettingsMessage::CancelFlavorSwitch),
            )))
            .into()
        } else {
            content
        }
    }
}

impl From<BitcoindSettingsState> for Box<dyn State> {
    fn from(s: BitcoindSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

/// A managed-node install payload: `(archive bytes, optional fetched
/// (SHA256SUMS, SHA256SUMS.asc) manifest)`. The manifest is `Some` for Knots
/// (verified against the manifest) and `None` for Core (verified by code hash).
type ManagedNodeInstall = (Vec<u8>, Option<(String, String)>);

/// Load-or-create the managed `bitcoin.conf`, apply `flavor` (RDTS enforcement),
/// ensure ports, apply any node-`resources` override, and write it back —
/// returning the `BitcoindConfig` (RPC endpoint) to connect to. Pure with respect
/// to process start (no `Bitcoind::maybe_start`), so the config-rewrite rules are
/// unit-testable; [`configure_and_start_internal_bitcoind`] calls this and then
/// launches bitcoind.
///
/// `resources: Some` overwrites `prune` on the (possibly pre-existing) network
/// section — keeping its ports/rpc_auth — and sets the global `max_mempool_mb`.
/// Without that overwrite, editing prune on an existing datadir would silently
/// no-op (the wholesale reuse of the existing section). `None` preserves whatever
/// is on disk untouched (it already round-trips through `from_file`/`to_file`).
fn write_internal_bitcoind_config(
    coincube_datadir: &CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    resources: Option<NodeResources>,
) -> Result<BitcoindConfig, String> {
    let bitcoind_datadir = internal_bitcoind_datadir(coincube_datadir);
    let config_path = internal_bitcoind_config_path(&bitcoind_datadir);

    let mut conf = match InternalBitcoindConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(InternalBitcoindConfigError::FileNotFound) => InternalBitcoindConfig::new(),
        Err(e) => return Err(e.to_string()),
    };
    // The chosen flavour drives RDTS enforcement: Knots emits
    // `consensusrules=rdts`, Core never does.
    conf.flavor = flavor;
    conf.enforce_rdts = matches!(flavor, NodeFlavor::Knots);

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

    let mut network_conf = existing.unwrap_or(InternalBitcoindNetworkConfig {
        rpc_port,
        p2p_port,
        prune: resources.map(|r| r.prune_mb).unwrap_or(PRUNE_DEFAULT),
        rpc_auth: None,
    });
    if let Some(r) = resources {
        network_conf.prune = r.prune_mb;
        conf.max_mempool_mb = r.max_mempool_mb;
    }
    conf.networks.insert(network, network_conf);
    conf.to_file(&config_path).map_err(|e| e.to_string())?;

    let cookie_path = internal_bitcoind_cookie_path(&bitcoind_datadir, &network);
    Ok(BitcoindConfig {
        rpc_auth: BitcoindRpcAuth::CookieFile(cookie_path),
        addr: internal_bitcoind_address(rpc_port),
    })
}

/// Configure and start an internally-managed pruned node of `flavor`.
/// If `install` is `Some((bytes, manifest))`, the binary is first verified and
/// installed from those bytes. Returns the `BitcoindConfig` and the live
/// `Bitcoind` handle (which keeps the lock file alive) to be stored by the
/// caller.
/// Clear the block-index rejection flags below the BIP-110 anchor on the managed
/// node, so it can follow the most-work chain again.
///
/// Blocking (opens an RPC connection), so callers must run it off the UI thread.
/// The `reconsiderblock` itself is fire-and-forget: re-activating the chain can
/// reconnect many blocks and outlast the RPC socket timeout, so we return as soon
/// as the request is away and let the node get on with it. Progress shows up in
/// the usual sync indicators.
fn repair_managed_node_chain(
    coincube_datadir: &CoincubeDirectory,
    cfg: &BitcoindConfig,
    network: Network,
) -> Result<String, String> {
    let anchor_height = crate::node::revalidate::rdts_anchor_height(network).ok_or_else(|| {
        "BIP-110 isn't deployed on this network, so there is nothing to repair.".to_string()
    })?;
    let bitcoind = coincubed::BitcoinD::new(cfg, "repair_node_chain".to_string())
        .map_err(|e| format!("Could not reach the managed node: {e}"))?;
    crate::node::revalidate::execute(
        coincube_datadir,
        &bitcoind,
        crate::node::revalidate::RevalidationPlan::ClearFailureFlags { anchor_height },
    )?;
    Ok("Asked the node to re-check its chain. This may take a few minutes.".to_string())
}

fn configure_and_start_internal_bitcoind(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    // When true, stop any running managed node first so the (re)start applies
    // the fresh config even at the same flavour — used by the "restart to apply"
    // action. `maybe_start` alone only restarts on a flavour change.
    force_restart: bool,
    // A node-resources edit (prune target + mempool cap). `Some` overwrites
    // `prune` on the (possibly pre-existing) network conf and sets the global
    // mempool cap; `None` preserves whatever is already on disk untouched.
    resources: Option<NodeResources>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    if let Some((bytes, manifest)) = install {
        let verification = DownloadVerification::for_flavor(flavor, manifest)
            .ok_or_else(|| "Missing release SHA256SUMS manifest for verification.".to_string())?;
        let install_dir = internal_bitcoind_directory(&coincube_datadir);
        install_bitcoind(&install_dir, &bytes, &verification).map_err(|e| format!("{:?}", e))?;
    }

    // Load-or-create + rewrite the managed `bitcoin.conf` (flavour/RDTS, ports,
    // and any resource override) and get the RPC endpoint to connect to.
    let bitcoind_config =
        write_internal_bitcoind_config(&coincube_datadir, network, flavor, resources)?;

    // Default-ON inbound-over-Tor for a freshly set-up enforcing (Knots) node on
    // mainnet — the point of the wedge is more reachable RDTS-enforcing nodes,
    // and it's mainnet-only. Only write the sidecar when the user hasn't already
    // made a choice, so an existing opt-out survives a reconfigure. The binary is
    // provisioned lazily on the next node start (see
    // `node::tor::ensure_tor_installed_if_wanted`).
    if matches!(flavor, NodeFlavor::Knots)
        && network == Network::Bitcoin
        && !crate::node::tor::InboundTorPreference::path(&coincube_datadir).exists()
    {
        if let Err(e) =
            crate::node::tor::InboundTorPreference::default_enabled().save(&coincube_datadir)
        {
            warn!("could not write default inbound-tor preference: {e}");
        }
    }

    // Force a clean restart when asked (same-flavour reconfigure), so the new
    // config below is actually applied rather than a running node reused.
    if force_restart {
        crate::node::bitcoind::stop_and_wait_managed_bitcoind(&bitcoind_config);
    }

    // Apply inbound-over-Tor before starting bitcoind: this starts the managed
    // Tor daemon and rewrites bitcoin.conf with the onion/proxy keys (mainnet
    // only, gated on the user's preference; fail-safe to outbound-only). Doing
    // it here means a flavour switch or a "restart to apply" picks the inbound
    // config up, matching the app-launch path in `loader`.
    crate::node::tor::prepare_inbound_tor(&coincube_datadir, network);

    let bitcoind = Bitcoind::maybe_start(network, bitcoind_config.clone(), &coincube_datadir)
        .map_err(|e| e.to_string())?;

    Ok((bitcoind_config, bitcoind))
}

/// Ensure the managed Tor binary is present (when inbound is wanted), then
/// configure and start the managed node on a blocking thread. Centralises the
/// tor-install + `spawn_blocking(configure_and_start_internal_bitcoind)` used by
/// the setup, flavour-switch, and restart flows so the blocking step's
/// `prepare_inbound_tor` can actually start Tor.
async fn ensure_tor_and_start_managed(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    force_restart: bool,
    resources: Option<NodeResources>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    crate::node::tor::ensure_tor_installed_if_wanted(&coincube_datadir).await;
    tokio::task::spawn_blocking(move || {
        configure_and_start_internal_bitcoind(
            coincube_datadir,
            network,
            flavor,
            install,
            force_restart,
            resources,
        )
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
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
    /// Flavour of the internal managed node — `Some` only when the node is the
    /// internal managed one — read from the on-disk `bitcoin.conf`. Drives the
    /// Core/Knots dropdown on the node card.
    managed_flavor: Option<NodeFlavor>,
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
        // For the internal managed node, recover its flavour from the on-disk
        // config so the node card can show a Core/Knots switcher.
        let managed_flavor = if bitcoind_is_internal {
            CoincubeDirectory::active().ok().and_then(|dir| {
                let cfg_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&dir));
                InternalBitcoindConfig::from_file(&cfg_path)
                    .ok()
                    .map(|c| c.flavor)
            })
        } else {
            None
        };
        BitcoindSettings {
            configured_node_type,
            daemon_is_external,
            bitcoind_is_internal,
            managed_flavor,
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
            // Handled by the parent `BitcoindSettingsState` (raises a
            // confirmation); never delegated here.
            view::SettingsEditMessage::SwitchManagedFlavor(_) => {}
        }
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
                self.managed_flavor,
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
        }
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
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonBackend, DaemonError};
    use crate::node::bitcoind::RpcAuth;
    use coincube_core::{
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{address, bip32::ChildNumber, psbt::Psbt, Address, OutPoint, Txid},
    };
    use coincubed::{
        bip329::Labels,
        commands::{CoinStatus, LabelItem, UpdateDerivIndexesResult},
        datadir::DataDirectory,
    };
    use std::collections::{HashMap, HashSet};

    const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    #[derive(Debug)]
    struct TestDaemon {
        config: Option<Config>,
    }

    #[async_trait::async_trait]
    impl Daemon for TestDaemon {
        fn backend(&self) -> DaemonBackend {
            DaemonBackend::EmbeddedCoincubed(Some(NodeType::Bitcoind))
        }

        fn config(&self) -> Option<&Config> {
            self.config.as_ref()
        }

        async fn is_alive(
            &self,
            _datadir: &CoincubeDirectory,
            _network: Network,
        ) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn stop(&self) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn get_info(&self) -> Result<crate::daemon::model::GetInfoResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn request_sync(&self) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn get_new_address(
            &self,
        ) -> Result<crate::daemon::model::GetAddressResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn list_revealed_addresses(
            &self,
            _is_change: bool,
            _exclude_used: bool,
            _limit: usize,
            _start_index: Option<ChildNumber>,
        ) -> Result<crate::daemon::model::ListRevealedAddressesResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn update_deriv_indexes(
            &self,
            _receive: Option<u32>,
            _change: Option<u32>,
        ) -> Result<UpdateDerivIndexesResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn list_coins(
            &self,
            _statuses: &[CoinStatus],
            _outpoints: &[OutPoint],
        ) -> Result<crate::daemon::model::ListCoinsResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn list_spend_txs(
            &self,
        ) -> Result<crate::daemon::model::ListSpendResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn create_spend_tx(
            &self,
            _coins_outpoints: &[OutPoint],
            _destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
            _feerate_vb: u64,
            _change_address: Option<Address<address::NetworkUnchecked>>,
        ) -> Result<crate::daemon::model::CreateSpendResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn rbf_psbt(
            &self,
            _txid: &Txid,
            _is_cancel: bool,
            _feerate_vb: Option<u64>,
        ) -> Result<crate::daemon::model::CreateSpendResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn update_spend_tx(&self, _psbt: &Psbt) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn delete_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn broadcast_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn start_rescan(&self, _t: u32) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn list_confirmed_txs(
            &self,
            _start: u32,
            _end: u32,
            _limit: u64,
        ) -> Result<crate::daemon::model::ListTransactionsResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn create_recovery(
            &self,
            _address: Address<address::NetworkUnchecked>,
            _coins_outpoints: &[OutPoint],
            _feerate_vb: u64,
            _sequence: Option<u16>,
        ) -> Result<Psbt, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn list_txs(
            &self,
            _txid: &[Txid],
        ) -> Result<crate::daemon::model::ListTransactionsResult, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn get_labels(
            &self,
            _labels: &HashSet<LabelItem>,
        ) -> Result<HashMap<String, String>, DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn update_labels(
            &self,
            _labels: &HashMap<LabelItem, Option<String>>,
        ) -> Result<(), DaemonError> {
            unreachable!("test daemon should not be queried")
        }

        async fn get_labels_bip329(
            &self,
            _offset: u32,
            _limit: u32,
        ) -> Result<Labels, DaemonError> {
            unreachable!("test daemon should not be queried")
        }
    }

    fn daemon(config: Option<Config>) -> Arc<dyn Daemon + Sync + Send> {
        Arc::new(TestDaemon { config })
    }

    fn bitcoin_config(network: Network) -> BitcoinConfig {
        BitcoinConfig {
            network,
            poll_interval_secs: std::time::Duration::from_secs(
                coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS,
            ),
        }
    }

    fn bitcoind_config(rpc_auth: BitcoindRpcAuth) -> BitcoindConfig {
        BitcoindConfig {
            rpc_auth,
            addr: "127.0.0.1:8332".parse().unwrap(),
        }
    }

    fn electrum_config() -> ElectrumConfig {
        ElectrumConfig {
            addr: "ssl://electrum.example:50002".to_string(),
            validate_domain: true,
        }
    }

    fn esplora_config() -> coincubed::config::EsploraConfig {
        coincubed::config::EsploraConfig {
            addr: "https://example.com/api".to_string(),
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        }
    }

    fn config_with_backend(backend: Option<BitcoinBackend>) -> Config {
        Config::new(
            bitcoin_config(Network::Bitcoin),
            backend,
            log::LevelFilter::Info,
            CoincubeDescriptor::from_str(DESC).unwrap(),
            DataDirectory::new(std::env::temp_dir().join("coincube-settings-state-test")),
        )
    }

    fn edit_message(message: view::SettingsEditMessage) -> Message {
        Message::View(view::Message::Settings(
            view::SettingsMessage::BitcoindSettings(message),
        ))
    }

    fn node_message(message: view::NodeSettingsMessage) -> Message {
        Message::View(view::Message::Settings(
            view::SettingsMessage::NodeSettings(message),
        ))
    }

    #[test]
    fn bitcoind_settings_new_hydrates_cookie_and_userpass_forms() {
        let cookie = BitcoindSettings::new(
            Some(NodeType::Bitcoind),
            bitcoin_config(Network::Bitcoin),
            bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
                "/tmp/bitcoin/.cookie",
            ))),
            false,
            false,
        );

        assert_eq!(cookie.selected_auth_type, RpcAuthType::CookieFile);
        assert_eq!(cookie.addr.value, "127.0.0.1:8332");
        assert_eq!(
            cookie.rpc_auth_vals.cookie_path.value,
            "/tmp/bitcoin/.cookie"
        );
        assert!(cookie.rpc_auth_vals.cookie_path.valid);
        assert!(!cookie.edit);
        assert!(!cookie.processing);

        let userpass = BitcoindSettings::new(
            Some(NodeType::Electrum),
            bitcoin_config(Network::Testnet),
            bitcoind_config(BitcoindRpcAuth::UserPass(
                "rpcuser".to_string(),
                "secret".to_string(),
            )),
            true,
            false,
        );

        assert_eq!(userpass.selected_auth_type, RpcAuthType::UserPass);
        assert_eq!(userpass.addr.value, "");
        assert_eq!(userpass.rpc_auth_vals.user.value, "rpcuser");
        assert_eq!(userpass.rpc_auth_vals.password.value, "secret");
        assert!(userpass.daemon_is_external);
    }

    #[test]
    fn edit_substates_toggle_editing_and_keep_processing_changes_guarded() {
        let cache = Cache::default();
        let daemon = daemon(None);
        let mut bitcoind = BitcoindSettings::new(
            Some(NodeType::Bitcoind),
            bitcoin_config(Network::Bitcoin),
            bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
                "/tmp/bitcoin/.cookie",
            ))),
            false,
            false,
        );

        let _ = bitcoind.update(daemon.clone(), &cache, view::SettingsEditMessage::Select);
        assert!(bitcoind.edit);
        let _ = bitcoind.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("socket_address", "bad".to_string()),
        );
        let _ = bitcoind.update(daemon.clone(), &cache, view::SettingsEditMessage::Confirm);
        assert!(!bitcoind.addr.valid);
        assert!(!bitcoind.processing);
        let _ = bitcoind.update(daemon.clone(), &cache, view::SettingsEditMessage::Cancel);
        assert!(!bitcoind.edit);

        bitcoind.processing = true;
        let previous = bitcoind.addr.value.clone();
        let _ = bitcoind.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("socket_address", "127.0.0.1:9999".to_string()),
        );
        assert_eq!(bitcoind.addr.value, previous);

        bitcoind.edited(false);
        assert!(!bitcoind.processing);
        bitcoind.edit = true;
        bitcoind.processing = true;
        bitcoind.edited(true);
        assert!(!bitcoind.edit);
        assert!(!bitcoind.processing);

        let mut electrum = ElectrumSettings::new(
            Some(NodeType::Electrum),
            bitcoin_config(Network::Bitcoin),
            electrum_config(),
            false,
        );
        let _ = electrum.update(daemon.clone(), &cache, view::SettingsEditMessage::Select);
        assert!(electrum.edit);
        let _ = electrum.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::ValidateDomainEdited(false),
        );
        assert!(!electrum.electrum_config.validate_domain);
        let _ = electrum.update(
            daemon,
            &cache,
            view::SettingsEditMessage::FieldEdited("address", "".to_string()),
        );
        assert!(!electrum.addr.valid);
        electrum.edited(true);
        assert!(!electrum.edit);
        assert!(!electrum.processing);
    }

    #[test]
    fn rescan_setting_validates_inputs_without_starting_daemon_on_bad_dates() {
        let cache = Cache::default();
        let daemon = daemon(None);
        let mut rescan = RescanSetting::new(Some(0.5));
        assert!(rescan.processing);

        rescan.edited(false);
        assert!(!rescan.processing);
        assert!(!rescan.success);

        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_year", "2024".to_string()),
        );
        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_month", "2".to_string()),
        );
        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_day", "31".to_string()),
        );
        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_year", "abcd".to_string()),
        );
        assert_eq!(rescan.year.value, "2024");

        let _ = rescan.update(daemon.clone(), &cache, view::SettingsEditMessage::Confirm);
        assert!(rescan.invalid_date);
        assert!(!rescan.processing);

        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_year", "2999".to_string()),
        );
        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_month", "1".to_string()),
        );
        let _ = rescan.update(
            daemon.clone(),
            &cache,
            view::SettingsEditMessage::FieldEdited("rescan_day", "1".to_string()),
        );
        let _ = rescan.update(daemon, &cache, view::SettingsEditMessage::Confirm);
        assert!(rescan.future_date);
        assert!(!rescan.processing);
    }

    #[test]
    fn state_new_selects_backend_substates_and_resource_defaults() {
        let cache = Cache::default();
        let bitcoind_state = BitcoindSettingsState::new(
            Some(config_with_backend(Some(BitcoinBackend::Bitcoind(
                bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
                    "/tmp/bitcoin/.cookie",
                ))),
            )))),
            &cache,
            false,
            false,
        );
        assert!(bitcoind_state.bitcoind_settings.is_some());
        assert!(bitcoind_state.electrum_settings.is_none());
        assert_eq!(
            bitcoind_state.node_prune_mb.value,
            PRUNE_DEFAULT.to_string()
        );
        assert_eq!(bitcoind_state.node_max_mempool_mb.value, "");

        let electrum_state = BitcoindSettingsState::new(
            Some(config_with_backend(Some(BitcoinBackend::Electrum(
                electrum_config(),
            )))),
            &cache,
            true,
            false,
        );
        assert!(electrum_state.bitcoind_settings.is_none());
        assert!(electrum_state.electrum_settings.is_some());

        let empty_state = BitcoindSettingsState::new(None, &cache, false, false);
        assert!(empty_state.bitcoind_settings.is_none());
        assert!(empty_state.electrum_settings.is_none());
    }

    #[test]
    fn state_update_tracks_setup_forms_flavor_confirmation_and_resource_presets() {
        let cache = Cache::default();
        let daemon = daemon(Some(config_with_backend(Some(BitcoinBackend::Esplora(
            esplora_config(),
        )))));
        let mut state = BitcoindSettingsState::new(
            Some(config_with_backend(Some(BitcoinBackend::Esplora(
                esplora_config(),
            )))),
            &cache,
            false,
            false,
        );

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNode),
        );
        assert!(matches!(
            state.pending_node_setup.as_ref().map(|s| s.mode),
            Some(None)
        ));

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeModeSelected(false)),
        );
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeAddrChanged(
                "bad".to_string(),
            )),
        );
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeConfirm),
        );
        assert_eq!(state.pending_node_setup.as_ref().unwrap().mode, Some(false));
        assert!(!state.pending_node_setup.as_ref().unwrap().addr.valid);

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeAuthTypeSelected(
                RpcAuthType::UserPass,
            )),
        );
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeAddrChanged(
                "127.0.0.1:8332".to_string(),
            )),
        );
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeFieldEdited(
                "password",
                "secret".to_string(),
            )),
        );
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeConfirm),
        );
        assert!(
            !state
                .pending_node_setup
                .as_ref()
                .unwrap()
                .rpc_auth_vals
                .user
                .valid
        );

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeCancel),
        );
        assert!(state.pending_node_setup.is_none());

        state.bitcoind_settings = Some(BitcoindSettings::new(
            Some(NodeType::Bitcoind),
            bitcoin_config(Network::Bitcoin),
            bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
                "/tmp/bitcoin/.cookie",
            ))),
            false,
            false,
        ));
        state.bitcoind_settings.as_mut().unwrap().managed_flavor = Some(NodeFlavor::Core);

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            edit_message(view::SettingsEditMessage::SwitchManagedFlavor(
                NodeFlavor::Knots,
            )),
        );
        assert_eq!(state.pending_flavor_switch, Some(NodeFlavor::Knots));
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::CancelFlavorSwitch),
        );
        assert_eq!(state.pending_flavor_switch, None);

        // Selecting the flavour the node already runs raises no confirmation:
        // there is nothing to switch, so the modal must not appear.
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            edit_message(view::SettingsEditMessage::SwitchManagedFlavor(
                NodeFlavor::Core,
            )),
        );
        assert_eq!(state.pending_flavor_switch, None);

        // The chain-repair escape hatch dispatches its work to a blocking task and
        // must not disturb any settings state on the way (in particular it must not
        // open a flavour-switch modal or a node-setup panel).
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::RepairNodeChain),
        );
        assert_eq!(state.pending_flavor_switch, None);
        assert!(state.pending_node_setup.is_none());

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::NodeResourceSmallComputer),
        );
        assert_eq!(
            state.node_prune_mb.value,
            NodeResources::small_computer().prune_mb.to_string()
        );
        assert_eq!(state.node_max_mempool_mb.value, "100");
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::NodeResourceRegularComputer),
        );
        assert_eq!(
            state.node_prune_mb.value,
            NodeResources::regular_computer().prune_mb.to_string()
        );
        assert_eq!(state.node_max_mempool_mb.value, "");
        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::NodeResourcePruneEdited(
                "100".to_string(),
            )),
        );
        assert!(!state.node_prune_mb.valid);
    }

    // A node-resources apply on a pre-existing datadir must overwrite prune and
    // the mempool cap while preserving the network section's ports and rpc_auth
    // (and the flavour/RDTS marker) — the "sharp edge" the settings path fixes. A
    // `None` apply must leave every value untouched.
    #[test]
    fn node_resources_apply_preserves_ports_and_rpcauth() {
        use std::fs;
        let base =
            std::env::temp_dir().join(format!("coincube-settings-noderes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));

        // Seed a pre-existing managed conf: Knots (RDTS on), fixed ports + a
        // user/pass rpc_auth, prune 15000, and an explicit 300 MB mempool.
        let rpc_auth: RpcAuth = "myuser:mysalt$myhmac".parse().unwrap();
        let mut seed = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        seed.max_mempool_mb = Some(300);
        seed.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 45001,
                p2p_port: 45002,
                prune: 15_000,
                rpc_auth: Some(rpc_auth.clone()),
            },
        );
        seed.to_file(&config_path).unwrap();

        // A `None` apply preserves everything (ports/rpc_auth/prune/mempool).
        let cfg_none =
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap();
        assert_eq!(cfg_none.addr.port(), 45001);
        let after_none = InternalBitcoindConfig::from_file(&config_path).unwrap();
        let net_none = after_none.networks.get(&Network::Bitcoin).unwrap();
        assert_eq!(net_none.prune, 15_000);
        assert_eq!(net_none.rpc_port, 45001);
        assert_eq!(net_none.p2p_port, 45002);
        assert_eq!(net_none.rpc_auth, Some(rpc_auth.clone()));
        assert_eq!(after_none.max_mempool_mb, Some(300));

        // A resource apply updates prune + mempool, keeps ports + rpc_auth + RDTS.
        let cfg = write_internal_bitcoind_config(
            &datadir,
            Network::Bitcoin,
            NodeFlavor::Knots,
            Some(NodeResources {
                prune_mb: 550,
                max_mempool_mb: Some(100),
            }),
        )
        .unwrap();
        assert_eq!(cfg.addr.port(), 45001); // reused the existing RPC port
        let after = InternalBitcoindConfig::from_file(&config_path).unwrap();
        let net = after.networks.get(&Network::Bitcoin).unwrap();
        assert_eq!(net.prune, 550); // updated
        assert_eq!(net.rpc_port, 45001); // preserved
        assert_eq!(net.p2p_port, 45002); // preserved
        assert_eq!(net.rpc_auth, Some(rpc_auth)); // preserved
        assert_eq!(after.max_mempool_mb, Some(100)); // updated
        assert!(after.enforce_rdts); // flavour/RDTS preserved

        let _ = fs::remove_dir_all(&base);
    }

    // Choosing the "Default" mempool (blank field) clears the cap back to the
    // key-omitted state, so the config returns to byte-identical-with-default.
    #[test]
    fn node_resources_default_mempool_clears_key() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "coincube-settings-noderes-def-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));

        let mut seed = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        seed.max_mempool_mb = Some(100);
        seed.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 46001,
                p2p_port: 46002,
                prune: 550,
                rpc_auth: None,
            },
        );
        seed.to_file(&config_path).unwrap();

        // Apply Default (max_mempool_mb = None) → the key is dropped.
        write_internal_bitcoind_config(
            &datadir,
            Network::Bitcoin,
            NodeFlavor::Core,
            Some(NodeResources {
                prune_mb: 15_000,
                max_mempool_mb: None,
            }),
        )
        .unwrap();
        let after = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert_eq!(after.max_mempool_mb, None);
        assert!(after.to_ini().general_section().get("maxmempool").is_none());
        assert_eq!(after.networks.get(&Network::Bitcoin).unwrap().prune, 15_000);

        let _ = fs::remove_dir_all(&base);
    }
}
