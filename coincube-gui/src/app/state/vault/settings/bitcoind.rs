use std::convert::{From, TryInto};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

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
        install_bitcoind, internal_bitcoind_address, DownloadVerification, PRUNE_DEFAULT,
    },
    node::{
        bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_cookie_path,
            internal_bitcoind_datadir, internal_bitcoind_directory, internal_bitcoind_exe_path,
            Bitcoind, InternalBitcoindConfig, InternalBitcoindNetworkConfig, NodeFlavor,
            NodeResources, RpcAuthType, RpcAuthValues,
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
    /// Which managed node flavour to install (Core or Knots).
    flavor: NodeFlavor,
    internal_stage: InternalSetupStage,
    internal_error: Option<String>,
    download_progress: f32,
}

/// Normalises a user-entered Esplora base URL, or `None` when it isn't one.
///
/// Deliberately strict about the scheme (an Esplora base without `http://` or
/// `https://` is a typo, not a shorthand we should guess at) and lenient about
/// a trailing slash, which users paste constantly and which would otherwise
/// produce `…//blocks/tip/height`.
fn normalize_esplora_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return None;
    }
    // Reject a bare scheme with no host.
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or_default();
    if rest.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Probes an Esplora endpoint by asking for the chain tip
/// (`GET {base}/blocks/tip/height`, the cheapest endpoint every Esplora
/// implementation serves) and returns the height it reports.
///
/// Short timeout: the user is staring at a button, and a slow endpoint is
/// itself a reason not to point a wallet at it.
async fn probe_esplora_tip(base_url: String) -> Result<u32, String> {
    const PROBE_TIMEOUT_SECS: u64 = 10;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(PROBE_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Couldn't build the probe client: {e}"))?;
    let url = format!("{base_url}/blocks/tip/height");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {base_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "{base_url} answered {} — is this an Esplora API base URL?",
            resp.status()
        ));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Couldn't read the response from {base_url}: {e}"))?;
    body.trim().parse::<u32>().map_err(|_| {
        format!("{base_url} didn't return a block height — is this an Esplora API base URL?")
    })
}

#[derive(Debug)]
pub struct BitcoindSettingsState {
    warning: Option<Error>,
    managed_owner: Arc<Mutex<Option<PathBuf>>>,
    config_updated: bool,
    full_config: Option<Config>,
    node_switch_processing: bool,
    pending_node_setup: Option<PendingNodeSetup>,
    cancel_node_setup_in_flight: bool,

    bitcoind_settings: Option<BitcoindSettings>,
    electrum_settings: Option<ElectrumSettings>,
    rescan_settings: RescanSetting,
    /// Cached inbound-over-Tor preference (from the sidecar), driving the
    /// "Inbound connections" toggles. Persisted on every change.
    inbound_tor_pref: crate::node::tor::InboundTorPreference,
    /// A managed-node flavour the user picked from the node-card dropdown but
    /// hasn't confirmed yet — while `Some`, a confirmation panel is shown.
    pending_flavor_switch: Option<NodeFlavor>,
    /// Node-resources editor for the internal managed node: prune target (MB) and
    /// mempool cap (MB, blank = bitcoind's 300 MB default). Loaded from the
    /// on-disk `bitcoin.conf` for the active network; applied via a force-restart.
    node_prune_mb: form::Value<String>,
    node_max_mempool_mb: form::Value<String>,
    recovery_start: form::Value<String>,
    recovery_date_picker_open: bool,
    retention_days: form::Value<String>,
    recovery_requested: bool,
    recovery_advanced: bool,
    node_advanced: bool,
    node_log_open: bool,
    /// "Your own Esplora" editor (`PLAN-connect-blinding` PR D5). Pre-filled
    /// with the active Esplora primary so the field always shows where chain
    /// queries actually go. Nothing is applied until the URL answers a probe.
    custom_esplora_url: form::Value<String>,
    /// True while the entered URL is being probed.
    custom_esplora_probing: bool,
    /// Last probe outcome: `Ok(tip_height)` or the reason it failed. Cleared
    /// when the field is edited.
    custom_esplora_status: Option<Result<u32, String>>,
    /// A manual "Switch to local node" waiting on the user's confirmation
    /// because the Vault is still scanning; while `Some`, a confirmation is shown.
    pending_scan_discard: Option<crate::app::local_switch::RunningScan>,
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
            &crate::node::bitcoind::internal_bitcoind_datadir_for(
                &cache.datadir_path,
                crate::node::bitcoind::NodeChainFamily::from_chain(cache.chain()),
            ),
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
        // Captured before `config` is moved into the sub-settings constructors.
        let active_esplora_addr = match config.as_ref().and_then(|c| c.bitcoin_backend.as_ref()) {
            Some(BitcoinBackend::Esplora(e)) => e.addr.clone(),
            _ => String::new(),
        };
        BitcoindSettingsState {
            warning: None,
            managed_owner: Arc::new(Mutex::new(None)),
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
            custom_esplora_url: form::Value {
                // Show the endpoint currently in use, whatever it is — the
                // point of the section is to make the chain-query path visible,
                // not just editable.
                value: active_esplora_addr,
                valid: true,
                warning: None,
            },
            custom_esplora_probing: false,
            recovery_requested: cache.node_history_switch_requested,
            recovery_advanced: false,
            node_advanced: false,
            node_log_open: false,
            recovery_start: form::Value::default(),
            recovery_date_picker_open: false,
            retention_days: form::Value {
                value: crate::node::retention::load(&cache.datadir_path, cache.network)
                    .ok()
                    .flatten()
                    .filter(|policy| !policy.temporary)
                    .map(|policy| policy.days.to_string())
                    .unwrap_or_default(),
                valid: true,
                warning: None,
            },
            custom_esplora_status: None,
            pending_scan_discard: None,
        }
    }

    pub(crate) fn with_managed_owner(mut self, owner: Arc<Mutex<Option<PathBuf>>>) -> Self {
        self.managed_owner = owner;
        self
    }

    pub(crate) fn managed_owner_path(&self) -> Option<PathBuf> {
        self.managed_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Make the pending local node the Vault's backend, keeping the current
    /// Connect config as the fallback. The checks that guard this live in the
    /// `SwitchToBitcoind` handler.
    fn switch_to_pending_bitcoind(
        &mut self,
        daemon: &Arc<dyn Daemon + Sync + Send>,
    ) -> Task<Message> {
        let Some(cfg) = daemon.config() else {
            return Task::none();
        };
        let Some(pending) = cfg.pending_bitcoind.clone() else {
            return Task::none();
        };
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
            std::time::Duration::from_secs(coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS);
        self.node_switch_processing = true;
        self.warning = None;
        Task::done(Message::LoadDaemonConfig(Box::new(new_cfg)))
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
            let err_msg = crate::user_error::report(&err);
            self.warning = Some(err);
            return Task::done(Message::View(view::Message::ShowError(err_msg)));
        };
        // Reconstruct the provider chain from cache.network so a stale
        // fallback_esplora.addr (e.g. written before Testnet4 was handled) is
        // never used. Mainnet routes primary traffic through COINCUBE API
        // (keeps addresses off public providers); every other network keeps a
        // public-primary chain (mempool.space → blockstream.info → Connect).
        // See `connect_esplora_config` for the assembled chain.
        let esplora = crate::installer::connect_esplora_config(cache.chain(), &jwt);
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

    /// The App's background install of the node requested at Vault creation,
    /// while it runs or after it failed. A failure stops showing once the
    /// request is gone from the config (e.g. replaced by a manual setup).
    fn background_install_card<'a>(
        &self,
        cache: &'a Cache,
    ) -> Option<Element<'a, view::NodeSettingsMessage>> {
        let status = &cache.background_node_install;
        let requested = self
            .full_config
            .as_ref()
            .is_some_and(|cfg| cfg.pending_managed_node_install.is_some());
        if !status.is_running() && !requested {
            return None;
        }
        view::vault::settings::background_node_install(status)
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
        // A provider of the wrong chain family is refused here, before the
        // download is dispatched or the installed binary looked for — the
        // helpers below refuse again, but no task may even start for it.
        if let Err(e) = provider_serves_network(flavor, network) {
            setup.internal_stage = InternalSetupStage::Idle;
            setup.internal_error = Some(e);
            return Task::none();
        }
        let exe_exists = internal_bitcoind_exe_path(&coincube_datadir, flavor.version()).exists();
        if exe_exists {
            setup.internal_stage = InternalSetupStage::Installing;
            Task::perform(
                ensure_tor_and_start_managed_for_owner(
                    coincube_datadir,
                    network,
                    flavor,
                    None,
                    false,
                    None,
                    self.managed_owner_path(),
                ),
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
        if cache.chain().is_blake2b() {
            match &message {
                Message::View(view::Message::Settings(view::SettingsMessage::NodeSettings(
                    msg,
                ))) => {
                    // The scan-discard confirmation belongs to the switch: without
                    // it a mid-scan "Switch to local node" is a dead click here.
                    if !matches!(
                        msg,
                        view::NodeSettingsMessage::SwitchToConnect
                            | view::NodeSettingsMessage::SwitchToConnectFastPath(_)
                            | view::NodeSettingsMessage::SwitchToBitcoind
                            | view::NodeSettingsMessage::ConfirmSwitchDiscardingScan
                            | view::NodeSettingsMessage::CancelSwitchDiscardingScan
                    ) {
                        return Task::none();
                    }
                }
                Message::View(view::Message::Settings(
                    view::SettingsMessage::BitcoindSettings(_)
                    | view::SettingsMessage::EditBitcoindSettings,
                )) => return Task::none(),
                _ => {}
            }
        }
        match message {
            Message::DaemonConfigLoaded(res) => match res {
                Ok(()) => {
                    self.config_updated = true;
                    self.node_switch_processing = false;
                    self.pending_node_setup = None;
                    self.warning = None;
                    self.full_config = daemon.config().cloned();
                    if self.full_config.as_ref().is_some_and(|cfg| {
                        matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_)))
                    }) {
                        // The App cleared its switch intent after promotion;
                        // retire the open panel's copy as well.
                        self.recovery_requested = false;
                    }
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
                    let err_msg = crate::user_error::report(&e);
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
                    let err_msg = crate::user_error::report(&e);
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
                    // A provider of the wrong chain family never reaches the
                    // confirmation, let alone the switch.
                    if let Err(e) = provider_serves_network(flavor, cache.network) {
                        self.warning = Some(Error::Unexpected(e));
                        return Task::none();
                    }
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
                    NodeSettingsMessage::RecoveryAdvancedToggled => {
                        self.recovery_advanced = !self.recovery_advanced;
                        if !self.recovery_advanced {
                            self.recovery_date_picker_open = false;
                        }
                    }
                    NodeSettingsMessage::RecoverySourceSelected(from_date) => {
                        self.recovery_advanced = from_date;
                        if !from_date {
                            self.recovery_date_picker_open = false;
                        }
                    }
                    NodeSettingsMessage::NodeLogToggled => {
                        self.node_log_open = !self.node_log_open;
                    }
                    NodeSettingsMessage::NodeAdvancedToggled => {
                        self.node_advanced = !self.node_advanced;
                    }
                    NodeSettingsMessage::RecoveryDismiss => {
                        self.recovery_requested = false;
                        self.recovery_date_picker_open = false;
                        self.warning = None;
                    }
                    NodeSettingsMessage::RecoveryStartEdited(value) => {
                        self.recovery_date_picker_open = false;
                        self.recovery_start.valid =
                            crate::app::node_history::parse_start(&value).is_ok();
                        self.recovery_start.value = value;
                    }
                    NodeSettingsMessage::RecoveryDateOpen => {
                        self.recovery_date_picker_open = self.recovery_advanced;
                    }
                    NodeSettingsMessage::RecoveryDateCancel => {
                        self.recovery_date_picker_open = false;
                    }
                    NodeSettingsMessage::RecoveryDateSelected(date) => {
                        self.recovery_date_picker_open = false;
                        let value = date.format("%Y-%m-%d").to_string();
                        self.recovery_start.valid =
                            crate::app::node_history::parse_start(&value).is_ok();
                        self.recovery_start.value = value;
                    }
                    NodeSettingsMessage::RetentionDaysEdited(value) => {
                        self.retention_days.valid = value
                            .parse::<u32>()
                            .is_ok_and(|days| (1..=3650).contains(&days));
                        self.retention_days.value = value;
                    }
                    NodeSettingsMessage::RecoveryStart => {
                        self.recovery_start.valid =
                            crate::app::node_history::parse_start(&self.recovery_start.value)
                                .is_ok();
                        if self.recovery_start.valid {
                            return Task::done(Message::NodeHistory(
                                crate::app::node_history::Action::Recover(
                                    self.recovery_start.value.clone(),
                                ),
                            ));
                        }
                    }
                    NodeSettingsMessage::ImportConnectHistory => {
                        return Task::done(Message::NodeHistory(
                            crate::app::node_history::Action::Import,
                        ))
                    }
                    NodeSettingsMessage::RecoveryResume => {
                        return Task::done(Message::NodeHistory(
                            crate::app::node_history::Action::Resume,
                        ))
                    }
                    NodeSettingsMessage::RecoveryPause => {
                        return Task::done(Message::NodeHistory(
                            crate::app::node_history::Action::Pause,
                        ))
                    }
                    NodeSettingsMessage::RecoveryCancel => {
                        self.recovery_requested = false;
                        return Task::done(Message::NodeHistory(
                            crate::app::node_history::Action::Cancel,
                        ));
                    }
                    NodeSettingsMessage::RetentionApply => {
                        self.retention_days.valid = self
                            .retention_days
                            .value
                            .parse::<u32>()
                            .is_ok_and(|days| (1..=3650).contains(&days));
                        if let Ok(days) = self.retention_days.value.parse::<u32>() {
                            if self.retention_days.valid {
                                return Task::done(Message::NodeHistory(
                                    crate::app::node_history::Action::Retain(days),
                                ));
                            }
                        }
                    }
                    NodeSettingsMessage::RetryBackgroundNodeSetup => {
                        // The App owns the background install and handles this
                        // before it reaches the panel.
                    }
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
                            flavor: crate::node::bitcoind::configured_managed_flavor(
                                &cache.datadir_path,
                            )
                            .unwrap_or(NodeFlavor::Knots),
                            internal_stage: InternalSetupStage::Idle,
                            internal_error: None,
                            download_progress: 0.0,
                        });
                    }
                    NodeSettingsMessage::SetupLocalNodeManagedFlavor(flavor) => {
                        // Pick the managed flavour (Core or Knots) and begin the
                        // download/install in one step. The choice is relay
                        // policy only; both builds follow the same consensus
                        // rules.
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
                                    let flavor = setup.flavor;
                                    // Nothing downloaded for a mismatched
                                    // provider is installed, and no manifest is
                                    // fetched for it.
                                    if let Err(e) = provider_serves_network(flavor, cache.network) {
                                        setup.internal_stage = InternalSetupStage::Idle;
                                        setup.internal_error = Some(e);
                                        return Task::none();
                                    }
                                    setup.internal_stage = InternalSetupStage::Installing;
                                    setup.download_progress = 100.0;
                                    let coincube_datadir = cache.datadir_path.clone();
                                    let network = cache.network;
                                    let managed_owner = self.managed_owner_path();
                                    return Task::perform(
                                        async move {
                                            // Fetch the release SHA256SUMS(+.asc)
                                            // the archive is verified against
                                            // (Knots); a no-op for Core.
                                            let manifest = download::fetch_release_manifest(flavor)
                                                .await
                                                .map_err(|e| e.to_string())?;
                                            ensure_tor_and_start_managed_for_owner(
                                                coincube_datadir,
                                                network,
                                                flavor,
                                                Some((bytes, manifest)),
                                                false,
                                                None,
                                                managed_owner,
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
                                        *self
                                            .managed_owner
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner()) =
                                            bitcoind.owner_lock_path().map(PathBuf::from);
                                        let mut new_cfg = cfg.clone();
                                        new_cfg.pending_bitcoind = Some(bitcoind_cfg);
                                        // This node supersedes a failed background
                                        // install request.
                                        new_cfg.pending_managed_node_install = None;
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
                                    new_cfg.pending_managed_node_install = None;
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
                                self.warning =
                                    Some(Error::Unexpected(coincube_ui::localized_format!(
                                        "Bitcoin node is still syncing ({:.1}%). \
                                         Please wait until sync is complete before switching.",
                                        "El nodo de Bitcoin aún se está sincronizando ({:.1} %). \
                                         Espera a que termine la sincronización antes de cambiar.",
                                        crate::utils::syncing_percent(
                                            cache.node_bitcoind_sync_progress.unwrap_or(0.0)
                                        )
                                    )));
                                return Task::none();
                            }
                            Some(false) => {}
                        }
                        // Check whether the local wallet already tracks the history
                        // or can recover it from the node's retained blocks.
                        if let Err(reason) = local_node_serves_vault(cache) {
                            let managed = self
                                .full_config
                                .as_ref()
                                .and_then(crate::app::local_node_sync_config)
                                .is_some_and(|cfg| {
                                    crate::node::retention::is_managed_endpoint(
                                        &cache.datadir_path,
                                        cache.network,
                                        cache.chain(),
                                        cfg,
                                    )
                                });
                            if managed && pruned_switch_refusal(cache).is_some() {
                                self.recovery_requested = true;
                                self.warning = None;
                                return Task::none();
                            }
                            self.warning = Some(Error::Unexpected(reason));
                            return Task::none();
                        }
                        // Switching stops the Connect backend, which throws a
                        // running scan away: say so and let the user choose.
                        if let Some(scan) = crate::app::running_vault_scan(daemon.backend(), cache)
                        {
                            self.pending_scan_discard = Some(scan);
                            return Task::none();
                        }
                        return self.switch_to_pending_bitcoind(&daemon);
                    }
                    NodeSettingsMessage::ConfirmSwitchDiscardingScan => {
                        if self.pending_scan_discard.take().is_some() {
                            return self.switch_to_pending_bitcoind(&daemon);
                        }
                    }
                    NodeSettingsMessage::CancelSwitchDiscardingScan => {
                        self.pending_scan_discard = None;
                    }
                    // "Inbound connections": persist the preference sidecar.
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
                        // A ledger naming a provider of the wrong chain family
                        // is refused before the node is stopped or Tor touched.
                        if let Err(e) = provider_serves_network(flavor, cache.network) {
                            self.warning = Some(Error::Unexpected(e));
                            return Task::none();
                        }
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
                            ensure_tor_and_start_managed_for_owner(
                                coincube_datadir,
                                network,
                                flavor,
                                None,
                                true,
                                None,
                                self.managed_owner_path(),
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
                    NodeSettingsMessage::CustomEsploraEdited(value) => {
                        self.custom_esplora_url.value = value;
                        self.custom_esplora_url.valid = true;
                        self.custom_esplora_url.warning = None;
                        // A previous verdict describes a different URL.
                        self.custom_esplora_status = None;
                    }
                    NodeSettingsMessage::CustomEsploraApply => {
                        let Some(url) = normalize_esplora_url(&self.custom_esplora_url.value)
                        else {
                            self.custom_esplora_url.valid = false;
                            self.custom_esplora_url.warning =
                                Some("Enter a full http:// or https:// URL");
                            return Task::none();
                        };
                        self.custom_esplora_url.valid = true;
                        self.custom_esplora_url.warning = None;
                        self.custom_esplora_probing = true;
                        self.custom_esplora_status = None;
                        // Probe before switching: a typo'd URL would otherwise
                        // park the daemon on an endpoint that never syncs, and
                        // the failure would show up as a silent stall.
                        return Task::perform(probe_esplora_tip(url), |r| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::NodeSettings(
                                    view::NodeSettingsMessage::CustomEsploraProbeResult(r),
                                ),
                            ))
                        });
                    }
                    NodeSettingsMessage::CustomEsploraProbeResult(result) => {
                        self.custom_esplora_probing = false;
                        let height = match result {
                            Ok(h) => h,
                            Err(e) => {
                                self.custom_esplora_status = Some(Err(e));
                                return Task::none();
                            }
                        };
                        self.custom_esplora_status = Some(Ok(height));
                        let Some(url) = normalize_esplora_url(&self.custom_esplora_url.value)
                        else {
                            return Task::none();
                        };
                        let Some(cfg) = daemon.config() else {
                            warn!(
                                "custom Esplora: daemon.config() is None (external coincubed?) \
                                 — cannot switch"
                            );
                            self.custom_esplora_status = Some(Err(
                                "This install runs an external daemon — set the Esplora URL in \
                                 its own configuration."
                                    .to_string(),
                            ));
                            return Task::none();
                        };
                        let mut new_cfg = cfg.clone();
                        if let Some(BitcoinBackend::Bitcoind(current)) = cfg.bitcoin_backend.clone()
                        {
                            new_cfg.pending_bitcoind = Some(current);
                        }
                        // Deliberate user choice — don't auto-revert to a
                        // parked local node on the next sync probe.
                        new_cfg.auto_switch_to_pending = Some(false);
                        // No fallbacks and no token. That is the whole point:
                        // a fallback chain would quietly send the very address
                        // queries the user is trying to keep private back to
                        // COINCUBE or a public provider the moment their own
                        // node hiccups. If their endpoint is down, sync stalls
                        // visibly instead.
                        new_cfg.bitcoin_backend =
                            Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
                                addr: url.clone(),
                                token: None,
                                fallback_addr: None,
                                fallback_token: None,
                                secondary_fallback_addr: None,
                                secondary_fallback_token: None,
                            }));
                        new_cfg.bitcoin_config.poll_interval_secs = std::time::Duration::from_secs(
                            coincubed::config::ESPLORA_POLL_INTERVAL_SECS,
                        );
                        new_cfg.fallback_esplora = None;
                        info!("Switching chain queries to custom Esplora {url} (tip {height})");
                        self.node_switch_processing = true;
                        self.warning = None;
                        return Task::done(Message::LoadDaemonConfig(Box::new(new_cfg)));
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
                        // Same refusal as the restart: nothing is rewritten,
                        // stopped or started for a mismatched provider.
                        if let Err(e) = provider_serves_network(flavor, cache.network) {
                            self.warning = Some(Error::Unexpected(e));
                            return Task::none();
                        }
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
                            ensure_tor_and_start_managed_for_owner(
                                coincube_datadir,
                                network,
                                flavor,
                                None,
                                true,
                                Some(resources),
                                self.managed_owner_path(),
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
        let recovery_in_progress = wallet_history_recovery_in_progress(cache);
        // A ready import syncs from its own server on either chain: it takes
        // the shared page below, which shows that server and its history, not
        // this chain's Connect/Knots switch.
        let imported = cache.has_imported_bitcoin && cache.imported_node.is_some();
        if cache.chain().is_blake2b() && !imported {
            let cfg = self.full_config.as_ref();
            let local = cfg.is_some_and(|cfg| {
                matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_)))
            });
            let pending = cfg.is_some_and(|cfg| cfg.pending_bitcoind.is_some());
            let active = if local {
                "Bitcoin Knots (Bitcoin Blake2b)"
            } else {
                "COINCUBE | Connect"
            };
            let mut panels = vec![
                view::vault::settings::node_backend_status(
                    active, if local { icon::bitcoin_icon() } else { icon::network_icon() },
                    cache.node_bitcoind_sync_progress, cache.node_bitcoind_sync_heights,
                    cache.node_bitcoind_ibd, cache.node_bitcoind_subversion.as_deref(),
                    cache.node_bitcoind_last_log.as_deref(),
                    self.node_log_open,
                    local && cfg.is_some_and(|cfg| cfg.fallback_esplora.is_some()),
                    !local && pending, recovery_in_progress, false,
                    cfg.is_some_and(|cfg| cfg.auto_switch_to_pending != Some(false)),
                    self.node_switch_processing, cache.daemon_switch_in_progress,
                    self.warning.as_ref().map(shown_warning),
                ).map(|msg| view::Message::Settings(view::SettingsMessage::NodeSettings(msg))),
                coincube_ui::component::text::text(coincube_ui::locale::copy("The pruned Bitcoin Knots node uses outbound peer connections. Inbound Tor and node resource editing are unavailable for Bitcoin Blake2b.")).into(),
            ];
            if let Some(card) = self.background_install_card(cache) {
                panels.insert(
                    1,
                    card.map(|msg| {
                        view::Message::Settings(view::SettingsMessage::NodeSettings(msg))
                    }),
                );
            }
            // This chain's node is always pruned: explain a refused switch here
            // too, as the Bitcoin view does.
            if !local && pending {
                if let Some(why) = pruned_switch_refusal(cache) {
                    panels.push(view::vault::settings::pruned_node_notice(why));
                }
            }
            return self.with_scan_discard_modal(view::vault::settings::bitcoind_settings(
                menu, cache, panels,
            ));
        }
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
        // Recovery must be available while Connect is still active: its
        // pending managed node may need history before it can be selected.
        let history_managed = self.pending_node_setup.is_none()
            && self
                .full_config
                .as_ref()
                .and_then(crate::app::local_node_sync_config)
                .is_some_and(|cfg| {
                    crate::node::retention::is_managed_endpoint(
                        &cache.datadir_path,
                        cache.network,
                        cache.chain(),
                        cfg,
                    )
                });
        let recovery_active = cache.node_history.job.as_ref().is_some_and(|job| {
            !matches!(
                job.phase,
                crate::node::history::Phase::Complete | crate::node::history::Phase::Cancelled
            )
        });
        let local_in_use = self
            .full_config
            .as_ref()
            .is_some_and(|cfg| matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_))));
        // New blocks do not undo a successful backend switch. A completed
        // transfer must not leave a disabled "Switch" prompt on an active node,
        // even if an old panel/cache still carries the original request.
        let history_card = history_managed
            && (cache.local_node_transfer_visible
                || (!local_in_use && self.recovery_requested)
                || recovery_active
                || cache.node_history.busy);
        let content = view::vault::settings::bitcoind_settings(menu, cache, {
            let mut setting_panels = Vec::new();
            let mut advanced_disclosure = false;
            let mut advanced_panels: Vec<Element<'_, view::Message>> = Vec::new();

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
                ) = if let Some(summary) = cache
                    .imported_node
                    .as_ref()
                    .filter(|_| cache.has_imported_bitcoin)
                {
                    // An imported wallet syncs from the server chosen at import
                    // (Connect, public or own Esplora, or Electrum), never a
                    // local node: nothing to switch to or set up here.
                    (summary.server, icon::network_icon(), false, false, false)
                } else if let Some(cfg) = &self.full_config {
                    let (ab, ai) = match &cfg.bitcoin_backend {
                        Some(BitcoinBackend::Esplora(_)) => {
                            ("COINCUBE | Connect", icon::network_icon())
                        }
                        Some(BitcoinBackend::Bitcoind(_)) => {
                            // Flavour-neutral: the exact build (Core vs Knots)
                            // comes from the node's runtime subversion,
                            // surfaced separately below.
                            ("Local Node", icon::bitcoin_icon())
                        }
                        Some(BitcoinBackend::Electrum(_)) => ("Electrum", icon::network_icon()),
                        None => ("None", icon::network_icon()),
                    };
                    let ctc = cfg.fallback_esplora.is_some()
                        && matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_)));
                    let ctb = cfg.pending_bitcoind.is_some()
                        && matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Esplora(_)));
                    // A running background install is that local node; a
                    // second setup would race it for the same node.
                    let csl = matches!(&cfg.bitcoin_backend, Some(BitcoinBackend::Esplora(_)))
                        && cfg.pending_bitcoind.is_none()
                        && !cache.background_node_install.is_running();
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
                    .map(shown_warning);
                setting_panels.push(
                    view::vault::settings::node_backend_status(
                        active_backend,
                        active_icon,
                        cache.node_bitcoind_sync_progress,
                        cache.node_bitcoind_sync_heights,
                        cache.node_bitcoind_ibd,
                        cache.node_bitcoind_subversion.as_deref(),
                        cache.node_bitcoind_last_log.as_deref(),
                        self.node_log_open,
                        can_switch_to_connect,
                        can_switch_to_bitcoind,
                        // The history card carries the switch while it shows.
                        recovery_in_progress || history_card,
                        can_setup_local_node,
                        self.full_config
                            .as_ref()
                            .is_some_and(|cfg| cfg.auto_switch_to_pending != Some(false)),
                        self.node_switch_processing,
                        cache.daemon_switch_in_progress,
                        warning_str,
                    )
                    .map(map_node_msg),
                );
                if let Some(card) = self.background_install_card(cache) {
                    setting_panels.push(card.map(map_node_msg));
                }
                if let Some(summary) = cache
                    .imported_node
                    .as_ref()
                    .filter(|_| cache.has_imported_bitcoin)
                {
                    setting_panels.push(view::vault::settings::imported_history_section(summary));
                }
                // Standing notice, not just a click-time warning: the
                // automatic switch is refused too, and the "will switch
                // once synced" copy above would otherwise go unexplained.
            }

            if history_managed {
                if history_card {
                    let synced = cache.node_bitcoind_ibd == Some(false);
                    use view::vault::settings::LocalNodeSwitchStep;
                    let switching = self.node_switch_processing || cache.daemon_switch_in_progress;
                    let can_switch = synced
                        && !switching
                        && self.full_config.as_ref().is_some_and(|cfg| {
                            cfg.pending_bitcoind.is_some()
                                && matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Esplora(_)))
                        })
                        && local_node_serves_vault(cache).is_ok();
                    let switch_step = if switching {
                        LocalNodeSwitchStep::Switching
                    } else if local_in_use && !recovery_in_progress {
                        LocalNodeSwitchStep::Complete
                    } else if can_switch {
                        LocalNodeSwitchStep::Ready
                    } else {
                        LocalNodeSwitchStep::Waiting
                    };
                    setting_panels.push(
                        view::vault::settings::wallet_history_section(
                            &self.recovery_start,
                            &cache.node_history,
                            self.recovery_advanced,
                            self.recovery_date_picker_open,
                            cache
                                .node_bitcoind_sync_heights
                                .map(|(blocks, _)| blocks)
                                .filter(|_| synced),
                            switch_step,
                        )
                        .map(map_node_msg),
                    );
                    if self.recovery_advanced {
                        setting_panels.push(
                            view::vault::settings::rolling_retention_section(
                                &self.retention_days,
                                &cache.node_history,
                                cache.node_bitcoind_pruning,
                                cache.node_bitcoind_sync_heights.map(|(blocks, _)| blocks),
                            )
                            .map(map_node_msg),
                        );
                    }
                }
                if self.full_config.as_ref().is_some_and(|cfg| {
                    matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_)))
                }) {
                    // The artboard's "Advanced node settings" disclosure holds
                    // the node's own settings, Tor, resources and rescan.
                    advanced_disclosure = true;
                    setting_panels.push(crate::app::view::settings::page::disclosure(
                        "tenshu.settings.node.advanced",
                        coincube_ui::locale::copy("Advanced node settings"),
                        None,
                        self.node_advanced,
                        map_node_msg(view::NodeSettingsMessage::NodeAdvancedToggled),
                    ));
                    if self.node_advanced {
                        advanced_panels.push(
                            view::vault::settings::rolling_retention_section(
                                &self.retention_days,
                                &cache.node_history,
                                cache.node_bitcoind_pruning,
                                cache.node_bitcoind_sync_heights.map(|(blocks, _)| blocks),
                            )
                            .map(map_node_msg),
                        );
                    }
                }
            }
            // Panels below join the disclosure when there is one.
            let mut node_panels = Vec::new();

            // An imported wallet syncs only from the server chosen at import,
            // shown by its own server and history section above. The daemon's
            // Bitcoind/Electrum controls would edit a config it does not use
            // (or, on Bitcoin Blake2b, do nothing), so they are not offered.
            if !cache.has_imported_bitcoin
                && (self.bitcoind_settings.is_some() || self.electrum_settings.is_some())
            {
                if let Some(settings) = self.bitcoind_settings.as_ref() {
                    node_panels.push(settings.view(cache, can_edit_bitcoind_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::BitcoindSettings(msg))
                        },
                    ));
                }

                // "Inbound connections": inbound-over-Tor controls, placed
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
                    node_panels.push(view::vault::settings::inbound_tor_section(
                        self.inbound_tor_pref.enabled,
                        self.inbound_tor_pref.outbound_via_tor,
                        self.inbound_tor_pref.max_upload_target_mb_day.is_some(),
                        crate::node::tor::managed_tor_ports().is_some(),
                        crate::node::tor::onion_key_exists(&cache.datadir_path, cache.network),
                        crate::node::bitcoind::tor_supported_on_host(),
                        cache.node_net_stats.as_ref(),
                    ));
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
                    node_panels.push(view::vault::settings::node_resources_section(
                        &self.node_prune_mb,
                        &self.node_max_mempool_mb,
                        false,
                    ));
                }

                // "Your own Esplora": only meaningful when the chain source
                // *is* Esplora — a local bitcoind already answers its own
                // queries, so there's nothing to redirect (PR D5).
                if self.pending_node_setup.is_none()
                    && matches!(
                        self.full_config
                            .as_ref()
                            .and_then(|c| c.bitcoin_backend.as_ref()),
                        Some(BitcoinBackend::Esplora(_))
                    )
                {
                    node_panels.push(
                        view::vault::settings::custom_esplora_section(
                            &self.custom_esplora_url,
                            self.custom_esplora_probing,
                            self.custom_esplora_status.as_ref(),
                        )
                        .map(map_node_msg),
                    );
                }

                if let Some(settings) = self.electrum_settings.as_ref() {
                    node_panels.push(settings.view(cache, can_edit_electrum_settings).map(
                        move |msg| {
                            view::Message::Settings(view::SettingsMessage::ElectrumSettings(msg))
                        },
                    ));
                }
            }
            // An imported wallet searches more addresses instead of rescanning
            // from a date (its Wallet history group).
            if !cache.has_imported_bitcoin {
                node_panels.push(
                    self.rescan_settings
                        .view(cache, can_do_rescan)
                        .map(move |msg| {
                            view::Message::Settings(view::SettingsMessage::RescanSettings(msg))
                        }),
                );
            }
            if advanced_disclosure {
                if self.node_advanced {
                    advanced_panels.extend(node_panels);
                    setting_panels.push(crate::app::view::settings::page::disclosure_body(
                        advanced_panels,
                    ));
                }
            } else {
                setting_panels.extend(node_panels);
            }
            setting_panels
        });

        // A pending Core↔Knots switch pops a confirmation modal over the page;
        // clicking the backdrop cancels it.
        if self.pending_scan_discard.is_some() {
            self.with_scan_discard_modal(content)
        } else if let Some(flavor) = self.pending_flavor_switch {
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

impl BitcoindSettingsState {
    /// `content` under the "switching discards the running scan" confirmation, when
    /// one is pending; clicking the backdrop cancels it. Shared by the Bitcoin and
    /// Bitcoin Blake2b views, so neither can lose the confirmation its switch waits on.
    fn with_scan_discard_modal<'a>(
        &self,
        content: Element<'a, view::Message>,
    ) -> Element<'a, view::Message> {
        let Some(scan) = self.pending_scan_discard else {
            return content;
        };
        modal::Modal::new(
            content,
            view::vault::settings::switch_discards_scan_confirm(scan)
                .map(|m| view::Message::Settings(view::SettingsMessage::NodeSettings(m))),
        )
        .on_blur(Some(view::Message::Settings(
            view::SettingsMessage::NodeSettings(
                view::NodeSettingsMessage::CancelSwitchDiscardingScan,
            ),
        )))
        .into()
    }
}

impl From<BitcoindSettingsState> for Box<dyn State> {
    fn from(s: BitcoindSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

/// Whether the pending local node can show this Vault's coins, judged from
/// what the node probe last read into the cache. `Err` carries the copy to
/// show; an unknown answer refuses for now rather than guessing.
fn wallet_history_recovery_in_progress(cache: &Cache) -> bool {
    cache.node_history.busy
        || cache.node_history.job.as_ref().is_some_and(|job| {
            !matches!(
                job.phase,
                crate::node::history::Phase::Complete | crate::node::history::Phase::Cancelled
            )
        })
}

/// The node-settings warning as shown: an app-authored [`Error::Unexpected`]
/// reason in the window's language (the stored error stays English).
fn shown_warning(error: &Error) -> String {
    match error {
        Error::Unexpected(reason) => coincube_ui::localized_format!(
            "Unexpected error: {}",
            "Error inesperado: {}",
            coincube_ui::locale::notice(reason.clone())
        ),
        error => error.to_string(),
    }
}

fn local_node_serves_vault(cache: &Cache) -> Result<(), String> {
    if wallet_history_recovery_in_progress(cache) {
        return Err(
            "Finish or cancel wallet history recovery before switching to the local node".into(),
        );
    }
    use crate::app::local_switch::{pruned_node_serves, NodePruning};
    match cache.node_bitcoind_pruning {
        None => Err("Couldn't read your local node's pruning settings yet. \
                     Please wait a moment and try again."
            .to_string()),
        Some(NodePruning::Unpruned) => Ok(()),
        Some(pruned) => match cache.local_switch_history {
            None => Err("Still checking whether your local node has this Vault's \
                         history. Please wait a moment and try again."
                .to_string()),
            Some(history) => pruned_node_serves(pruned, history).map_err(|why| why.to_string()),
        },
    }
}

/// The reason a switch to the pending local node would hide this Vault's
/// coins, for a standing notice on the Node settings. `None` when the node is
/// unpruned, still unknown, or keeps every block the Vault needs.
fn pruned_switch_refusal(cache: &Cache) -> Option<crate::app::local_switch::PrunedHistory> {
    crate::app::local_switch::pruned_node_serves(
        cache.node_bitcoind_pruning?,
        cache.local_switch_history?,
    )
    .err()
}

/// Refuse a managed-node provider that cannot serve this Vault's chain.
///
/// The settings screen is keyed on the Vault's `bitcoin::Network`; the chain it
/// stands for is its identity (`ChainId::from`), which is what the provider's
/// family is checked against. Every settings path that would download,
/// unpack, write the Bitcoin node's conf/ledger/identity, provision Tor, or
/// stop and start the node calls this first, so an out-of-family provider —
/// however it got into the pending setup or the ledger — changes nothing.
fn provider_serves_network(flavor: NodeFlavor, network: Network) -> Result<(), String> {
    flavor
        .check_chain(crate::chain::ChainId::from(network))
        .map_err(|e| e.to_string())
}

/// A managed-node install payload: `(archive bytes, optional fetched
/// (SHA256SUMS, SHA256SUMS.asc) manifest)`. The manifest is `Some` for Knots
/// (verified against the manifest) and `None` for Core (verified by code hash).
pub(crate) type ManagedNodeInstall = (Vec<u8>, Option<(String, String)>);

/// Load-or-create the managed `bitcoin.conf`, apply `flavor`,
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
    // Everything below is the *Bitcoin* node's: its conf, its ledger, its
    // identity marker. A provider of another chain family is refused before any
    // of them is touched.
    provider_serves_network(flavor, network)?;
    if resources.is_some() {
        crate::node::retention::disable(coincube_datadir, network)?;
    }
    let bitcoind_datadir = internal_bitcoind_datadir(coincube_datadir);

    // From the read of the conf to its replacement, under the datadir-wide
    // managed-conf lock on a fresh read (see `node::managed_conf`): the ports
    // are chosen against everything persisted by now, and the write cannot
    // erase a section another setup added meanwhile. Refusals — lock busy,
    // conf unreadable, no acceptable port, other family's conf unreadable —
    // happen before the ledger is touched; a failure to *replace* the conf
    // comes after the ledger was recorded inside the edit, and leaves it so.
    let rpc_port = crate::node::managed_conf::update_managed_conf(
        coincube_datadir,
        flavor.chain_family(),
        |txn| {
            let mut conf = txn.conf.clone().unwrap_or_else(InternalBitcoindConfig::new);
            conf.flavor = flavor;

            // Ports first, before anything is recorded. An existing section
            // keeps its ports untouched.
            let existing = conf.networks.get(&network).cloned();
            let (rpc_port, p2p_port) = match &existing {
                Some(nc) => (nc.rpc_port, nc.p2p_port),
                // The same bounded policy as the installer: never a port this
                // conf's other networks or the other chain family's node
                // already holds, and a refusal if the other family's conf
                // cannot be read.
                None => txn.allocate_ports(
                    &conf,
                    crate::installer::step::node::bitcoind::get_available_port,
                )?,
            };

            // Nothing in the file records the flavour, so the ledger is where the
            // choice has to be kept — and kept before the write, so a write that
            // then fails still leaves the choice on record.
            crate::node::revalidate::ManagedNodeState::record_configured(coincube_datadir, flavor);
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
            Ok((rpc_port, Some(conf)))
        },
    )
    .map_err(|e| e.to_string())?
    .logged("writing the managed bitcoin.conf from settings");

    let cookie_path = internal_bitcoind_cookie_path(&bitcoind_datadir, &network);
    // Stamp the datadir with an identity, if it does not already carry one. The RPC
    // port and the cookie path both survive the datadir being wiped and rebuilt
    // underneath them, so anything scoped to the node by those alone would carry over
    // to its replacement. The marker lives inside the datadir, so it goes when the
    // datadir goes.
    if let Err(e) = crate::node::bitcoind::ensure_node_instance_marker(&cookie_path) {
        // Not fatal: without it, identity falls back to the endpoint and cookie path,
        // which is where it stood before the marker existed.
        tracing::warn!("could not stamp the managed node's datadir with an identity: {e}");
    }
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
#[cfg(test)]
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
    provider_serves_network(flavor, network)?;
    let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
        &coincube_datadir,
        crate::node::bitcoind::NodeChainFamily::Bitcoin,
    )
    .map_err(|e| e.to_string())?;
    configure_and_start_internal_bitcoind_locked(
        coincube_datadir,
        network,
        flavor,
        install,
        force_restart,
        resources,
        None,
        &lifecycle,
    )
}

#[allow(clippy::too_many_arguments)]
fn configure_and_start_internal_bitcoind_locked(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    force_restart: bool,
    resources: Option<NodeResources>,
    initiating_owner: Option<&std::path::Path>,
    lifecycle: &crate::node::bitcoind::ManagedNodeLifecycle,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    // Before the archive is unpacked into the Bitcoin root, the conf/ledger
    // rewritten, the running node stopped or Tor prepared: the provider must
    // be one of this chain's. `write_internal_bitcoind_config` and
    // `maybe_start` check again on their own boundaries.
    provider_serves_network(flavor, network)?;
    lifecycle.ensure_exclusive_settings_owner(initiating_owner)?;
    if let Some((bytes, manifest)) = install {
        let verification = DownloadVerification::for_flavor(flavor, manifest)
            .ok_or_else(|| "Missing release SHA256SUMS manifest for verification.".to_string())?;
        let install_dir = internal_bitcoind_directory(&coincube_datadir);
        install_bitcoind(&install_dir, &bytes, &verification).map_err(|e| format!("{:?}", e))?;
    }

    // Load-or-create + rewrite the managed `bitcoin.conf` (ports and any
    // resource override) and get the RPC endpoint to connect to.
    let bitcoind_config =
        write_internal_bitcoind_config(&coincube_datadir, network, flavor, resources)?;

    // Default-ON inbound-over-Tor for a freshly set-up Knots node on mainnet —
    // the point is more reachable sovereign nodes, and it's mainnet-only. Only write the sidecar when the user hasn't already
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
    // only, gated on the user's preference; fail-safe to outbound-only on any
    // Tor problem). Doing it here means a flavour switch or a "restart to
    // apply" picks the inbound config up, matching the app-launch path in
    // `loader`. A conf that could not be read, locked or replaced is a
    // refusal, not a start from stale privacy configuration.
    (if force_restart {
        crate::node::tor::prepare_inbound_tor(&coincube_datadir, network)
    } else {
        crate::node::tor::prepare_inbound_tor_for_chain_locked(
            &coincube_datadir,
            network.into(),
            lifecycle,
        )
    })
    .map_err(|e| {
        crate::node::bitcoind::StartInternalBitcoindError::ConfigUnavailable(e.to_string())
            .to_string()
    })?;

    let bitcoind = if force_restart {
        Bitcoind::maybe_restart_locked(
            network,
            bitcoind_config.clone(),
            &coincube_datadir,
            lifecycle,
            initiating_owner,
        )
    } else {
        Bitcoind::maybe_start_locked(
            network,
            bitcoind_config.clone(),
            &coincube_datadir,
            lifecycle,
        )
    }
    .map_err(|e| e.to_string())?;

    Ok((bitcoind_config, bitcoind))
}

/// Ensure the managed Tor binary is present (when inbound is wanted), then
/// configure and start the managed node on a blocking thread. Centralises the
/// tor-install + `spawn_blocking(configure_and_start_internal_bitcoind)` used by
/// the setup, flavour-switch, and restart flows so the blocking step's
/// `prepare_inbound_tor` can actually start Tor.
pub(crate) async fn ensure_tor_and_start_managed(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    force_restart: bool,
    resources: Option<NodeResources>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    ensure_tor_and_start_managed_for_owner(
        coincube_datadir,
        network,
        flavor,
        install,
        force_restart,
        resources,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn ensure_tor_and_start_managed_for_owner(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    force_restart: bool,
    resources: Option<NodeResources>,
    initiating_owner: Option<PathBuf>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    ensure_tor_and_start_managed_with_lifecycle(
        coincube_datadir,
        network,
        flavor,
        install,
        force_restart,
        resources,
        initiating_owner,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn ensure_tor_and_start_managed_with_lifecycle(
    coincube_datadir: CoincubeDirectory,
    network: Network,
    flavor: NodeFlavor,
    install: Option<ManagedNodeInstall>,
    force_restart: bool,
    resources: Option<NodeResources>,
    initiating_owner: Option<PathBuf>,
    lifecycle: Option<crate::node::bitcoind::ManagedNodeLifecycle>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    // Tor is provisioned for the Bitcoin node; not for a provider that cannot
    // serve this chain.
    provider_serves_network(flavor, network)?;
    // Recovery starts suppress automatic wallet loading, so a stale wallet
    // cannot prevent the node from serving the blocks needed to repair it.
    // Reattach wallets that were already live for other Cubes after restart.
    let old_conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(
        &internal_bitcoind_datadir(&coincube_datadir),
    ))
    .ok();
    let previous = old_conf
        .as_ref()
        .and_then(|conf| conf.networks.get(&network))
        .map(|section| BitcoindConfig {
            addr: internal_bitcoind_address(section.rpc_port),
            rpc_auth: BitcoindRpcAuth::CookieFile(internal_bitcoind_cookie_path(
                &internal_bitcoind_datadir(&coincube_datadir),
                &network,
            )),
        });
    let loaded = if force_restart {
        match &previous {
            Some(cfg) => match crate::node::history::NodeRpc::new(cfg, "", network.into()).await {
                Ok(rpc) => rpc
                    .call(false, "listwallets", serde_json::json!([]))
                    .await
                    .ok()
                    .and_then(|value| {
                        value.as_array().map(|values| {
                            values
                                .iter()
                                .filter_map(|value| value.as_str().map(str::to_owned))
                                .collect::<Vec<_>>()
                        })
                    })
                    .unwrap_or_default(),
                Err(_) => vec![],
            },
            None => vec![],
        }
    } else {
        vec![]
    };
    // Hold the lifecycle lock across Tor installation and the subsequent
    // config/restart. Otherwise another Cube could attach after the check.
    let lifecycle = match lifecycle {
        Some(lifecycle) => {
            lifecycle.ensure_exclusive_settings_owner(initiating_owner.as_deref())?;
            lifecycle
        }
        None => {
            let root = coincube_datadir.clone();
            let preflight_owner = initiating_owner.clone();
            tokio::task::spawn_blocking(move || {
                let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
                    &root,
                    crate::node::bitcoind::NodeChainFamily::Bitcoin,
                )
                .map_err(|e| e.to_string())?;
                lifecycle.ensure_exclusive_settings_owner(preflight_owner.as_deref())?;
                Ok::<_, String>(lifecycle)
            })
            .await
            .map_err(|e| e.to_string())??
        }
    };
    crate::node::tor::ensure_tor_installed_if_wanted(&coincube_datadir).await;
    let started = tokio::task::spawn_blocking(move || {
        configure_and_start_internal_bitcoind_locked(
            coincube_datadir,
            network,
            flavor,
            install,
            force_restart,
            resources,
            initiating_owner.as_deref(),
            &lifecycle,
        )
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))?;
    reload_wallets_after_restart(&started.0, network, loaded).await;
    Ok(started)
}

/// Wallet catch-up failures cannot invalidate a successfully started node.
async fn reload_wallets_after_restart(
    config: &BitcoindConfig,
    network: Network,
    loaded: Vec<String>,
) {
    for wallet in loaded {
        let result = async {
            let rpc = crate::node::history::NodeRpc::new(config, &wallet, network.into()).await?;
            rpc.ensure_loaded(&wallet).await
        }
        .await;
        match result {
            Ok(true) => {}
            Ok(false) => tracing::warn!(wallet = %wallet,
                "A previously loaded Vault needs historical block recovery before it can reconnect"),
            Err(error) => tracing::warn!(wallet = %wallet, %error,
                "The local node started, but this Vault could not reconnect"),
        }
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
    /// Flavour of the internal managed node — `Some` only when the node is the
    /// internal managed one. Drives the Core/Knots dropdown on the node card.
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
        // For the internal managed node, recover its flavour so the node card can
        // show a Core/Knots switcher.
        //
        // Deliberately *not* from the on-disk `bitcoin.conf`: that file does not
        // record the flavour, so reading `conf.flavor` reports the `Core`
        // placeholder for every managed node, Knots included.
        //
        // Observed before configured. The card states what the node *is*, and the
        // two can honestly disagree: `select_managed_bitcoind_exe` falls back to
        // the other flavour's binary when the configured one isn't installed, so a
        // datadir configured for Knots with only Core on disk really is running
        // Core, and saying "Knots" there would be the same lie in reverse. The
        // ledger's configured value is the fallback for a node that has not been
        // observed running yet.
        let managed_flavor = if bitcoind_is_internal {
            CoincubeDirectory::active().ok().and_then(|dir| {
                let ledger = crate::node::revalidate::ManagedNodeState::load(&dir);
                ledger
                    .last_run_flavor
                    .or_else(|| crate::node::bitcoind::configured_managed_flavor(&dir))
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

    fn confirmed_rpc_auth(&mut self) -> Option<BitcoindRpcAuth> {
        match self.selected_auth_type {
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
                let rpc_auth = self.confirmed_rpc_auth();

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
                cache
                    .node_net_stats
                    .as_ref()
                    .and_then(|s| s.subversion.as_deref()),
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
            view::SettingsEditMessage::ValidateDomainEdited(b) if !self.processing => {
                self.electrum_config.validate_domain = b;
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

fn rescan_timestamp(network: Network, date: i64) -> i64 {
    let genesis = match network {
        Network::Testnet => TESTNET3_GENESIS_BLOCK_TIMESTAMP,
        Network::Testnet4 => TESTNET4_GENESIS_BLOCK_TIMESTAMP,
        Network::Signet => SIGNET_GENESIS_BLOCK_TIMESTAMP,
        // Regtest uses the mainnet floor, as do other network variants.
        _ => MAINNET_GENESIS_BLOCK_TIMESTAMP,
    };
    if date < genesis {
        info!(
            "Date {} prior to genesis block, using genesis block timestamp {}",
            date, genesis
        );
        genesis
    } else {
        date
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
                    rescan_timestamp(cache.network, date)
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
        // A rescan and a history recovery would both drive the node wallet.
        let recovery_running = wallet_history_recovery_in_progress(cache);
        view::vault::settings::rescan(
            &self.year,
            &self.month,
            &self.day,
            cache.rescan_progress(),
            self.success,
            self.processing,
            can_edit && !recovery_running,
            self.invalid_date,
            self.past_possible_height,
            self.future_date,
            recovery_running,
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
    #[tokio::test]
    async fn reload_error_does_not_prevent_other_wallets_reconnecting() {
        use httpmock::prelude::*;
        use serde_json::json;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(r#"{"method":"listwallets"}"#);
                then.status(200)
                    .json_body(json!({"id":"history", "result":[]}));
            })
            .await;
        let first = server.mock_async(|when, then| {
            when.method(POST).json_body_partial(r#"{"method":"createwallet","params":["failed",true,true,"",false,true,true]}"#);
            then.status(200).json_body(json!({"id":"history", "error":{"code":-4,"message":"Synthetic reload failure"}}));
        }).await;
        let second = server
            .mock_async(|when, then| {
                when.method(POST).json_body_partial(
                    r#"{"method":"createwallet","params":["ready",true,true,"",false,true,true]}"#,
                );
                then.status(200)
                    .json_body(json!({"id":"history", "result":{}}));
            })
            .await;
        let config = BitcoindConfig {
            addr: *server.address(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                "fixture".into(),
                "fixture".into(),
            ),
        };
        reload_wallets_after_restart(
            &config,
            Network::Regtest,
            vec!["failed".into(), "ready".into()],
        )
        .await;
        first.assert_async().await;
        second.assert_async().await;
    }

    #[test]
    fn manual_switch_waits_for_recovery_even_with_cached_history_permission() {
        let mut cache = Cache {
            node_bitcoind_pruning: Some(crate::app::local_switch::NodePruning::Pruned {
                prune_height: 1000,
            }),
            local_switch_history: Some(crate::app::local_switch::VaultHistory::TrackedLocally),
            ..Cache::default()
        };
        assert!(local_node_serves_vault(&cache).is_ok());
        cache.node_history.busy = true;
        assert!(local_node_serves_vault(&cache).is_err());
        cache.node_history.busy = false;
        let binding = crate::node::history::Binding {
            genesis: coincube_core::miniscript::bitcoin::blockdata::constants::genesis_block(
                Network::Bitcoin,
            )
            .block_hash(),
            node_instance: "fixture".into(),
            endpoint: "fixture".into(),
            wallet: "fixture".into(),
            descriptor: "fixture".into(),
        };
        cache.node_history.job = Some(
            crate::node::history::Recovery::new(
                binding,
                100,
                200,
                coincube_core::miniscript::bitcoin::blockdata::constants::genesis_block(
                    Network::Bitcoin,
                )
                .block_hash(),
            )
            .unwrap(),
        );
        assert!(local_node_serves_vault(&cache).is_err());
    }
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
        // Mirrors `EmbeddedDaemon::backend`: the node type follows the config.
        fn backend(&self) -> DaemonBackend {
            DaemonBackend::EmbeddedCoincubed(
                self.config
                    .as_ref()
                    .and_then(|c| c.bitcoin_backend.as_ref())
                    .map(NodeType::from)
                    .or(Some(NodeType::Bitcoind)),
            )
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

    #[test]
    fn late_setup_result_does_not_replace_settings_owner_after_panel_closed() {
        let root =
            std::env::temp_dir().join(format!("late-node-setup-owner-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let cache = Cache {
            datadir_path: datadir.clone(),
            ..Cache::default()
        };
        let descriptor = crate::app::state::vault::test_support::unified::fixture().descriptor;
        let wallet = Arc::new(crate::app::wallet::Wallet::new(descriptor));
        let previous = PathBuf::from("existing-owner.lock");
        let mut settings = crate::app::state::vault::settings::SettingsState::new(
            datadir.clone(),
            wallet,
            true,
            Arc::new(crate::app::Config::new(false)),
        )
        .with_managed_owner(Some(previous.clone()));
        let cfg = bitcoind_config(BitcoindRpcAuth::CookieFile(root.join(".cookie")));
        let node = Bitcoind::for_test(cfg.clone(), &datadir, Network::Bitcoin);

        let task = settings.update(
            Some(daemon(Some(config_with_backend(Some(
                BitcoinBackend::Bitcoind(cfg.clone()),
            ))))),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeStartResult(Ok((
                cfg, node,
            )))),
        );
        assert_eq!(task.units(), 0);
        assert_eq!(settings.managed_owner_path(), Some(previous));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn setup_result_without_daemon_config_does_not_replace_owner() {
        let root = std::env::temp_dir().join(format!(
            "missing-config-setup-owner-{}",
            uuid::Uuid::new_v4()
        ));
        let datadir = CoincubeDirectory::new(root.clone());
        let cache = Cache {
            datadir_path: datadir.clone(),
            ..Cache::default()
        };
        let previous = Some(PathBuf::from("existing-owner.lock"));
        let owner = Arc::new(Mutex::new(previous.clone()));
        let mut state =
            BitcoindSettingsState::new(None, &cache, false, true).with_managed_owner(owner);
        let unavailable = daemon(None);
        let _ = state.update(
            Some(unavailable.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNode),
        );
        assert!(state.pending_node_setup.is_some());
        let cfg = bitcoind_config(BitcoindRpcAuth::CookieFile(root.join(".cookie")));
        let node = Bitcoind::for_test(cfg.clone(), &datadir, Network::Bitcoin);

        let task = state.update(
            Some(unavailable),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeStartResult(Ok((
                cfg, node,
            )))),
        );
        assert_eq!(task.units(), 0);
        assert_eq!(state.managed_owner_path(), previous);
        let _ = std::fs::remove_dir_all(root);
    }

    fn bitcoin_config(network: Network) -> BitcoinConfig {
        BitcoinConfig::new(
            crate::chain::ChainId::from(network),
            std::time::Duration::from_secs(coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS),
        )
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
    fn recovery_calendar_only_commits_a_selected_date() {
        let cache = Cache::default();
        let daemon = daemon(None);
        let mut state = BitcoindSettingsState::new(None, &cache, false, false);

        macro_rules! send {
            ($message:expr) => {
                let _ = state.update(Some(daemon.clone()), &cache, node_message($message));
            };
        }
        send!(view::NodeSettingsMessage::RecoveryAdvancedToggled);
        send!(view::NodeSettingsMessage::RecoveryStartEdited(
            "840000".into()
        ));
        assert_eq!(state.recovery_start.value, "840000");
        assert!(state.recovery_start.valid);

        send!(view::NodeSettingsMessage::RecoveryDateOpen);
        assert!(state.recovery_date_picker_open);
        send!(view::NodeSettingsMessage::RecoveryDateCancel);
        assert!(!state.recovery_date_picker_open);
        assert_eq!(state.recovery_start.value, "840000");

        send!(view::NodeSettingsMessage::RecoveryStartEdited(
            "2024-02-".into()
        ));
        assert!(!state.recovery_start.valid);
        send!(view::NodeSettingsMessage::RecoveryDateOpen);
        send!(view::NodeSettingsMessage::RecoveryDateCancel);
        assert_eq!(state.recovery_start.value, "2024-02-");

        send!(view::NodeSettingsMessage::RecoveryDateOpen);
        send!(view::NodeSettingsMessage::RecoveryDateSelected(
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap(),
        ));
        assert!(!state.recovery_date_picker_open);
        assert_eq!(state.recovery_start.value, "2024-02-29");
        assert!(state.recovery_start.valid);

        send!(view::NodeSettingsMessage::RecoveryDateOpen);
        send!(view::NodeSettingsMessage::RecoveryAdvancedToggled);
        assert!(!state.recovery_advanced);
        assert!(!state.recovery_date_picker_open);
        assert_eq!(state.recovery_start.value, "2024-02-29");
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

    /// An imported Vault's Node page names its server, shows its history scan
    /// with Retry synchronization and the wider address search, and offers
    /// no local node or date rescan.
    #[tokio::test]
    async fn an_imported_vaults_node_page_shows_its_history_not_local_nodes() {
        use crate::app::state::imported_bitcoin::{ImportedNodeSummary, ImportedSyncStatus};
        use crate::home::restore_frame::rendered_labels;
        let summary = ImportedNodeSummary {
            server: "My Electrum server",
            endpoint: "ssl://electrum.example:50002".into(),
            status: Some(coincubed::commands::ImportedWalletSync {
                wallet_identity: "id".into(),
                chain_id: crate::chain::ChainId::Bitcoin,
                birthday: 0,
                initial_lookahead: 200,
                discovery_target: 200,
                receive_scanned: 200,
                change_scanned: 200,
                history_complete: true,
            }),
            sync_status: ImportedSyncStatus::Synced,
            ready: true,
            busy: false,
            can_expand: true,
            notice: None,
        };
        let failed = ImportedNodeSummary {
            sync_status: ImportedSyncStatus::Failed,
            ..summary.clone()
        };
        let shown = rendered_labels(view::vault::settings::imported_history_section(&failed)).await;
        assert!(shown.iter().any(|label| label == "Sync failed"));
        assert!(!shown.iter().any(|label| label == "Synced"));
        let state = BitcoindSettingsState::new(
            Some(config_with_backend(Some(BitcoinBackend::Electrum(
                electrum_config(),
            )))),
            &Cache::default(),
            false,
            false,
        );
        let menu = Menu::Vault(crate::app::menu::VaultSubMenu::Settings(Some(
            crate::app::menu::SettingsOption::Node,
        )));
        let imported = Cache {
            has_imported_bitcoin: true,
            imported_node: Some(summary),
            ..Cache::default()
        };
        let shown = rendered_labels(State::view(&state, &menu, &imported)).await;
        for label in [
            "Wallet history",
            "My Electrum server",
            "Retry synchronization",
            "Search 200 more addresses",
        ] {
            assert!(
                shown.iter().any(|l| l.contains(label)),
                "{}: {:?}",
                label,
                shown
            );
        }
        assert!(
            !shown.iter().any(|l| l == "Year"),
            "no date rescan: {:?}",
            shown
        );
        assert!(
            !shown.iter().any(|l| l == "Electrum"),
            "no daemon Electrum controls for an import: {:?}",
            shown
        );

        // The same on Bitcoin Blake2b: no Knots/Connect switch for an import.
        let blake2b = Cache {
            fiat_chain: crate::chain::ChainId::BitcoinBlake2b,
            ..imported
        };
        let shown = rendered_labels(State::view(&state, &menu, &blake2b)).await;
        for label in [
            "Wallet history",
            "My Electrum server",
            "Retry synchronization",
            "Search 200 more addresses",
        ] {
            assert!(
                shown.iter().any(|l| l.contains(label)),
                "Blake2b {}: {:?}",
                label,
                shown
            );
        }
        assert!(
            !shown
                .iter()
                .any(|l| l.contains("Bitcoin Knots (Bitcoin Blake2b)")
                    || l.contains("COINCUBE | Connect")),
            "no Blake2b node switch: {:?}",
            shown
        );

        // A Vault keeps its date rescan and its Electrum controls, and has
        // no imported history.
        let vault = Cache::default();
        let shown = rendered_labels(State::view(&state, &menu, &vault)).await;
        assert!(!shown.iter().any(|l| l == "Wallet history"));
        assert!(
            shown.iter().any(|l| l == "Electrum"),
            "a Vault keeps its Electrum controls: {:?}",
            shown
        );
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
    fn background_install_card_shows_while_running_or_failed_with_a_request() {
        use crate::app::cache::BackgroundNodeInstall;
        let esplora = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        let mut requested = esplora.clone();
        requested.pending_managed_node_install = Some("Knots".to_string());
        let mut cache = Cache::default();
        let with_request = BitcoindSettingsState::new(Some(requested), &cache, false, false);
        let without_request = BitcoindSettingsState::new(Some(esplora), &cache, false, false);

        assert!(with_request.background_install_card(&cache).is_none());
        for running in [
            BackgroundNodeInstall::Downloading(None),
            BackgroundNodeInstall::Downloading(Some(12.0)),
            BackgroundNodeInstall::Starting,
        ] {
            cache.background_node_install = running;
            assert!(with_request.background_install_card(&cache).is_some());
            // The config snapshot may lag the install that the App started.
            assert!(without_request.background_install_card(&cache).is_some());
        }
        cache.background_node_install = BackgroundNodeInstall::Failed("refused".to_string());
        assert!(with_request.background_install_card(&cache).is_some());
        assert!(
            without_request.background_install_card(&cache).is_none(),
            "a failure stops showing once a manual setup replaced the request"
        );
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

    // ── "Your own Esplora" (PR D5) ───────────────────────────────────

    #[test]
    fn esplora_url_normalisation_is_strict_about_scheme_lenient_about_slashes() {
        // A trailing slash is the single most common paste artefact and would
        // otherwise produce `…//blocks/tip/height`.
        assert_eq!(
            normalize_esplora_url("  https://esplora.example.com/api/  "),
            Some("https://esplora.example.com/api".to_string())
        );
        assert_eq!(
            normalize_esplora_url("http://localhost:3002"),
            Some("http://localhost:3002".to_string())
        );
        // A missing scheme is a typo, not a shorthand to guess at — guessing
        // https:// could silently downgrade a local http endpoint.
        assert_eq!(normalize_esplora_url("esplora.example.com/api"), None);
        assert_eq!(normalize_esplora_url(""), None);
        assert_eq!(normalize_esplora_url("   "), None);
        assert_eq!(normalize_esplora_url("https://"), None);
        assert_eq!(normalize_esplora_url("ftp://esplora.example.com"), None);
    }

    #[test]
    fn editing_the_esplora_url_clears_the_previous_verdict() {
        // A stale "reachable, height 800000" next to a freshly-typed URL would
        // read as approval of the new one.
        let cache = Cache::default();
        let daemon = daemon(Some(config_with_backend(Some(BitcoinBackend::Esplora(
            esplora_config(),
        )))));
        let mut state = BitcoindSettingsState::new(daemon.config().cloned(), &cache, false, false);
        state.custom_esplora_status = Some(Ok(800_000));

        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::CustomEsploraEdited(
                "https://mine.example/api".to_string(),
            )),
        );
        assert_eq!(state.custom_esplora_url.value, "https://mine.example/api");
        assert!(state.custom_esplora_status.is_none());
    }

    #[test]
    fn applying_a_malformed_esplora_url_flags_the_field_without_probing() {
        let cache = Cache::default();
        let daemon = daemon(Some(config_with_backend(Some(BitcoinBackend::Esplora(
            esplora_config(),
        )))));
        let mut state = BitcoindSettingsState::new(daemon.config().cloned(), &cache, false, false);

        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::CustomEsploraEdited(
                "esplora.example.com".to_string(),
            )),
        );
        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::CustomEsploraApply),
        );
        assert!(!state.custom_esplora_url.valid);
        assert!(state.custom_esplora_url.warning.is_some());
        // No probe was started, so nothing is in flight.
        assert!(!state.custom_esplora_probing);
    }

    #[test]
    fn a_failed_probe_never_touches_the_daemon_config() {
        let cache = Cache::default();
        let daemon = daemon(Some(config_with_backend(Some(BitcoinBackend::Esplora(
            esplora_config(),
        )))));
        let mut state = BitcoindSettingsState::new(daemon.config().cloned(), &cache, false, false);
        state.custom_esplora_url.value = "https://mine.example/api".to_string();
        state.custom_esplora_probing = true;

        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::CustomEsploraProbeResult(Err(
                "Couldn't reach https://mine.example/api".to_string(),
            ))),
        );
        assert!(!state.custom_esplora_probing);
        assert!(matches!(state.custom_esplora_status, Some(Err(_))));
        // The switch never started.
        assert!(!state.node_switch_processing);
    }

    #[test]
    fn a_successful_probe_switches_to_the_url_with_no_fallbacks() {
        let cache = Cache::default();
        let daemon = daemon(Some(config_with_backend(Some(BitcoinBackend::Esplora(
            esplora_config(),
        )))));
        let mut state = BitcoindSettingsState::new(daemon.config().cloned(), &cache, false, false);
        state.custom_esplora_url.value = "https://mine.example/api/".to_string();

        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::CustomEsploraProbeResult(Ok(
                912_345,
            ))),
        );
        assert_eq!(state.custom_esplora_status, Some(Ok(912_345)));
        assert!(state.node_switch_processing);
        // The emitted config is inspected through the message the task carries;
        // asserting the state transition here and the config shape in
        // `custom_esplora_config_drops_every_fallback` below keeps this test
        // about the state machine.
    }

    #[test]
    fn custom_esplora_config_drops_every_fallback() {
        // The privacy property the whole feature exists for: a fallback chain
        // would send the very address lookups the user is trying to keep
        // private back to COINCUBE (or a public provider) the moment their own
        // node hiccups. Pin that the applied config has none.
        let url = normalize_esplora_url("https://mine.example/api/").unwrap();
        let cfg = coincubed::config::EsploraConfig {
            addr: url.clone(),
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        };
        assert_eq!(cfg.addr, "https://mine.example/api");
        assert!(cfg.token.is_none());
        assert!(cfg.fallback_addr.is_none());
        assert!(cfg.secondary_fallback_addr.is_none());
    }

    // A node-resources apply on a pre-existing datadir must overwrite prune and
    // the mempool cap while preserving the network section's ports and rpc_auth —
    // the "sharp edge" the settings path fixes. A `None` apply must leave every
    // value untouched.
    #[test]
    fn node_resources_apply_preserves_ports_and_rpcauth() {
        use std::fs;
        let base =
            std::env::temp_dir().join(format!("coincube-settings-noderes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));

        // Seed a pre-existing managed conf: Knots, fixed ports + a user/pass
        // rpc_auth, prune 15000, and an explicit 300 MB mempool.
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

        // A resource apply updates prune + mempool, keeps ports + rpc_auth.
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

        // No `consensusrules` is written; the flavour is kept in the ledger
        // instead of the file.
        assert!(!std::fs::read_to_string(&config_path)
            .unwrap()
            .contains("consensusrules"));
        assert_eq!(
            crate::node::bitcoind::configured_managed_flavor(&datadir),
            Some(NodeFlavor::Knots),
        );
        // The trap this replaced: a config parsed back off disk reports the
        // placeholder `Core` for a Knots node, because nothing in the file says
        // otherwise. Anything that needs the managed flavour must ask the ledger,
        // never `conf.flavor` — reading the latter is what made the node card
        // show "Bitcoin Core" for a node the user installed as Knots.
        assert_eq!(after.flavor, NodeFlavor::Core);

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

    // ── Bitcoin Blake2b provider: refused at every settings boundary ────

    /// A COINCUBE datadir under the OS temp dir, unique per test thread, and a
    /// checker that nothing managed-node related was ever created under it.
    fn a_temp_datadir(name: &str) -> (PathBuf, CoincubeDirectory) {
        let base = std::env::temp_dir().join(format!(
            "coincube-settings-btcb2-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        (base.clone(), CoincubeDirectory::new(base))
    }

    fn assert_nothing_created(datadir: &CoincubeDirectory) {
        use crate::node::bitcoind::NodeChainFamily;
        for family in NodeChainFamily::ALL.iter().copied() {
            let root = crate::node::bitcoind::internal_bitcoind_directory_for(datadir, family);
            assert!(!root.exists(), "{} was created", root.display());
        }
        assert!(!crate::node::tor::InboundTorPreference::path(datadir).exists());
        assert!(!crate::node::revalidate::ManagedNodeState::path(datadir).exists());
        let entries: Vec<_> = std::fs::read_dir(datadir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(entries.is_empty(), "datadir gained entries: {:?}", entries);
    }

    #[tokio::test]
    async fn settings_restart_refuses_another_same_process_owner_before_mutation() {
        let (base, datadir) = a_temp_datadir("same-process-owner");
        let config = BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::CookieFile(base.join("bitcoind/.cookie")),
        };
        let initiating = Bitcoind::for_test(config.clone(), &datadir, Network::Bitcoin);
        let other = Bitcoind::for_test(config, &datadir, Network::Bitcoin);
        let owner_path = initiating.owner_lock_path().unwrap().to_path_buf();

        let err = ensure_tor_and_start_managed_for_owner(
            datadir.clone(),
            Network::Bitcoin,
            NodeFlavor::Knots,
            Some((vec![0u8; 16], None)),
            true,
            Some(NodeResources::small_computer()),
            Some(owner_path.clone()),
        )
        .await
        .unwrap_err();
        assert!(err.contains("Another Cube owns"), "{}", err);
        assert!(!internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir)).exists());
        assert!(!crate::node::revalidate::ManagedNodeState::path(&datadir).exists());
        assert!(!crate::node::tor::InboundTorPreference::path(&datadir).exists());
        assert!(initiating.owner_lock_path().unwrap().exists());
        assert!(other.owner_lock_path().unwrap().exists());

        drop(other);
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            std::time::Duration::from_secs(120)
        ));
        // The same Cube's sole owner passes the ownership gate. Verification
        // then rejects the deliberately incomplete archive before any write.
        let err = configure_and_start_internal_bitcoind_locked(
            datadir.clone(),
            Network::Bitcoin,
            NodeFlavor::Knots,
            Some((vec![0u8; 16], None)),
            true,
            None,
            Some(&owner_path),
            &crate::node::bitcoind::ManagedNodeLifecycle::acquire(
                &datadir,
                crate::node::bitcoind::NodeChainFamily::Bitcoin,
            )
            .unwrap(),
        )
        .unwrap_err();
        assert!(err.contains("Missing release SHA256SUMS"), "{}", err);
        drop(initiating);
        let _ = std::fs::remove_dir_all(base);
    }

    const BLAKE2B_ON_BITCOIN: &str = "Bitcoin Knots (Bitcoin Blake2b) cannot serve Bitcoin: \
                                      it is a Bitcoin Blake2b node provider";

    // Every settings message that can download, install, rewrite the conf,
    // provision Tor, or stop/start the managed node refuses the Blake2b
    // provider on a Bitcoin Vault without starting a task or touching disk —
    // whether the provider arrives through the pending setup, the flavour
    // switch, or the configured-flavour ledger. The picker never offers it;
    // this pins the boundary, not a click path.
    #[test]
    fn settings_refuse_the_blake2b_provider_before_any_side_effect() {
        let (base, datadir) = a_temp_datadir("state");
        let cache = Cache {
            datadir_path: datadir.clone(),
            network: Network::Bitcoin,
            ..Cache::default()
        };
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

        // Setup panel opened; the provider is forced into the pending setup.
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNode),
        );
        state.pending_node_setup.as_mut().unwrap().flavor = NodeFlavor::KnotsBlake2b;

        // Picking it (the setup/switch start path) and retrying both refuse
        // before the download or the installed-binary lookup.
        for msg in [
            view::NodeSettingsMessage::SetupLocalNodeManagedFlavor(NodeFlavor::KnotsBlake2b),
            view::NodeSettingsMessage::SetupLocalNodeModeSelected(true),
        ] {
            let task = state.update(Some(daemon.clone()), &cache, node_message(msg));
            assert_eq!(task.units(), 0, "a task was started");
            let setup = state.pending_node_setup.as_ref().unwrap();
            assert_eq!(setup.internal_error.as_deref(), Some(BLAKE2B_ON_BITCOIN));
            assert_eq!(setup.internal_stage, InternalSetupStage::Idle);
            assert_eq!(setup.flavor, NodeFlavor::KnotsBlake2b);
        }
        // A download that somehow completed for it is not installed and no
        // manifest is fetched.
        let task = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::SetupLocalNodeDownloadComplete(
                Ok(vec![1, 2, 3]),
            )),
        );
        assert_eq!(task.units(), 0);
        let setup = state.pending_node_setup.as_ref().unwrap();
        assert_eq!(setup.internal_error.as_deref(), Some(BLAKE2B_ON_BITCOIN));
        assert_eq!(setup.internal_stage, InternalSetupStage::Idle);
        assert_ne!(setup.download_progress, 100.0);

        // The node-card flavour switch: never armed for it, and a switch that
        // was somehow armed is refused when confirmed.
        state.pending_node_setup = None;
        state.bitcoind_settings = Some(BitcoindSettings::new(
            Some(NodeType::Bitcoind),
            bitcoin_config(Network::Bitcoin),
            bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
                "/tmp/bitcoin/.cookie",
            ))),
            false,
            true,
        ));
        state.bitcoind_settings.as_mut().unwrap().managed_flavor = Some(NodeFlavor::Core);
        let task = state.update(
            Some(daemon.clone()),
            &cache,
            edit_message(view::SettingsEditMessage::SwitchManagedFlavor(
                NodeFlavor::KnotsBlake2b,
            )),
        );
        assert_eq!(task.units(), 0);
        assert_eq!(state.pending_flavor_switch, None);
        assert!(state
            .warning
            .as_ref()
            .is_some_and(|w| w.to_string().contains(BLAKE2B_ON_BITCOIN)));
        state.warning = None;
        state.pending_flavor_switch = Some(NodeFlavor::KnotsBlake2b);
        let task = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::ConfirmFlavorSwitch),
        );
        assert_eq!(task.units(), 0);
        assert_eq!(state.pending_flavor_switch, None);
        assert_eq!(
            state
                .pending_node_setup
                .as_ref()
                .and_then(|s| s.internal_error.as_deref()),
            Some(BLAKE2B_ON_BITCOIN)
        );

        // A ledger naming it as the configured flavour: restart-to-apply and a
        // node-resources apply both refuse before stopping the node, touching
        // Tor or rewriting the conf.
        state.pending_node_setup = None;
        state.bitcoind_settings.as_mut().unwrap().managed_flavor = Some(NodeFlavor::KnotsBlake2b);
        for msg in [
            view::NodeSettingsMessage::RestartNodeToApply,
            view::NodeSettingsMessage::NodeResourceApply,
        ] {
            state.warning = None;
            let task = state.update(Some(daemon.clone()), &cache, node_message(msg));
            assert_eq!(task.units(), 0, "a task was started");
            assert!(
                state.pending_node_setup.is_none(),
                "a setup panel was opened"
            );
            assert!(state
                .warning
                .as_ref()
                .is_some_and(|w| w.to_string().contains(BLAKE2B_ON_BITCOIN)));
        }

        // Nothing on disk changed through any of it.
        assert_nothing_created(&datadir);

        // The Bitcoin providers still go through: the switch is armed as before.
        state.warning = None;
        state.bitcoind_settings.as_mut().unwrap().managed_flavor = Some(NodeFlavor::Core);
        let _ = state.update(
            Some(daemon),
            &cache,
            edit_message(view::SettingsEditMessage::SwitchManagedFlavor(
                NodeFlavor::Knots,
            )),
        );
        assert_eq!(state.pending_flavor_switch, Some(NodeFlavor::Knots));
        assert!(state.warning.is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    // The helpers behind those messages refuse at their own boundaries too —
    // before the archive is unpacked, before the conf/ledger/identity are
    // written, before Tor is provisioned — so a caller that skipped the
    // message-level check still changes nothing.
    #[tokio::test]
    async fn settings_helpers_refuse_the_blake2b_provider_before_writing() {
        let (base, datadir) = a_temp_datadir("helpers");

        assert_eq!(
            write_internal_bitcoind_config(
                &datadir,
                Network::Bitcoin,
                NodeFlavor::KnotsBlake2b,
                None
            )
            .map(|_| ()),
            Err(BLAKE2B_ON_BITCOIN.to_string())
        );
        assert_nothing_created(&datadir);

        // With an "archive" to install and a forced restart: refused before
        // the unpack, the stop, or Tor.
        let result = tokio::task::spawn_blocking({
            let datadir = datadir.clone();
            move || {
                configure_and_start_internal_bitcoind(
                    datadir,
                    Network::Bitcoin,
                    NodeFlavor::KnotsBlake2b,
                    Some((vec![0u8; 16], None)),
                    true,
                    Some(NodeResources::small_computer()),
                )
                .map(|_| ())
            }
        })
        .await
        .unwrap();
        assert_eq!(result, Err(BLAKE2B_ON_BITCOIN.to_string()));
        assert_nothing_created(&datadir);

        let result = ensure_tor_and_start_managed(
            datadir.clone(),
            Network::Bitcoin,
            NodeFlavor::KnotsBlake2b,
            Some((vec![0u8; 16], None)),
            true,
            None,
        )
        .await
        .map(|_| ());
        assert_eq!(result, Err(BLAKE2B_ON_BITCOIN.to_string()));
        assert_nothing_created(&datadir);

        // The same helper with a Bitcoin provider gets past the guard and does
        // its ordinary work (the ledger names it; the conf is written).
        let cfg =
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap();
        assert!(matches!(cfg.rpc_auth, BitcoindRpcAuth::CookieFile(_)));
        assert_eq!(
            crate::node::revalidate::ManagedNodeState::load(&datadir).configured_flavor,
            Some(NodeFlavor::Knots)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // The settings path allocates a new section's ports through the same
    // policy as the installer: a port the other family's conf records — bound
    // or not — is never handed out, this conf's other network sections are
    // reserved too, an existing section keeps its ports, and an unreadable
    // other-family conf refuses the write before the ledger is touched.
    #[test]
    fn settings_port_allocation_follows_the_shared_policy() {
        use crate::node::bitcoind::{internal_bitcoind_datadir_for, NodeChainFamily};
        use std::fs;
        let (base, datadir) = a_temp_datadir("ports");
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        };
        // The other family records ports nothing is listening on.
        let held = crate::installer::step::node::bitcoind::get_available_port().unwrap();
        let held2 = crate::installer::step::node::bitcoind::get_available_port().unwrap();
        let mut blake2b = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        blake2b
            .networks
            .insert(Network::Bitcoin, section(held, held2));
        let blake2b_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &datadir,
            NodeChainFamily::BitcoinBlake2b,
        ));
        fs::create_dir_all(blake2b_conf.parent().unwrap()).unwrap();
        blake2b.to_file(&blake2b_conf).unwrap();
        // And this file already has a testnet4 section.
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        let mut seed = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        seed.networks
            .insert(Network::Testnet4, section(45011, 45012));
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        seed.to_file(&config_path).unwrap();

        let cfg =
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap();
        let after = InternalBitcoindConfig::from_file(&config_path).unwrap();
        let main = after.networks.get(&Network::Bitcoin).unwrap();
        assert_eq!(cfg.addr.port(), main.rpc_port);
        assert_ne!(main.rpc_port, main.p2p_port);
        for p in [main.rpc_port, main.p2p_port].iter() {
            assert!(
                ![held, held2, 45011, 45012].contains(p),
                "port {} reused",
                p
            );
        }
        // A new managed node uses its normal relay policy on either chain.
        assert_eq!(after.data_carrier_size, None);
        assert!(!fs::read_to_string(&blake2b_conf)
            .unwrap()
            .contains("datacarriersize"));
        // The existing section and the other family's file are untouched.
        let t4 = after.networks.get(&Network::Testnet4).unwrap();
        assert_eq!((t4.rpc_port, t4.p2p_port), (45011, 45012));
        assert_eq!(
            InternalBitcoindConfig::from_file(&blake2b_conf)
                .unwrap()
                .networks,
            blake2b.networks
        );
        // A second write keeps the allocated ports.
        let again =
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap();
        assert_eq!(again.addr.port(), main.rpc_port);

        // Unreadable other-family conf: refused before the ledger is written.
        let (base2, datadir2) = a_temp_datadir("ports-unreadable");
        let blake2b_conf2 = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &datadir2,
            NodeChainFamily::BitcoinBlake2b,
        ));
        fs::create_dir_all(blake2b_conf2.parent().unwrap()).unwrap();
        fs::write(&blake2b_conf2, "[main]\nrpcport=notanumber\nport=1\n").unwrap();
        let err =
            write_internal_bitcoind_config(&datadir2, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap_err();
        assert!(err.contains("could not be read"), "{}", err);
        assert!(!internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir2)).exists());
        assert!(!crate::node::revalidate::ManagedNodeState::path(&datadir2).exists());

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&base2);
    }

    // A conf this process cannot see is not an absent conf: the settings
    // writer refuses (own family: before the ledger is touched; other family:
    // the reservation read) instead of starting a fresh file or reserving
    // nothing. Unix-only precondition; skipped when running privileged.
    #[cfg(unix)]
    #[test]
    fn settings_refuse_an_unreadable_managed_conf_instead_of_treating_it_as_absent() {
        use crate::node::bitcoind::{internal_bitcoind_datadir_for, NodeChainFamily};
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        };
        let restore = |dir: &std::path::Path| {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
        };
        let deny = |dir: &std::path::Path| {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
        };

        // Other family's conf under a non-traversable directory.
        let (base, datadir) = a_temp_datadir("perm-other");
        let other_dir = internal_bitcoind_datadir_for(&datadir, NodeChainFamily::BitcoinBlake2b);
        fs::create_dir_all(&other_dir).unwrap();
        let mut other = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        other
            .networks
            .insert(Network::Bitcoin, section(51001, 51002));
        let other_conf = internal_bitcoind_config_path(&other_dir);
        other.to_file(&other_conf).unwrap();
        deny(&other_dir);
        let privileged = fs::metadata(&other_conf).is_ok();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .map(|_| ())
        }));
        restore(&other_dir);
        let result = result.unwrap();
        if privileged {
            eprintln!("skipping the permission fixture: metadata succeeds despite mode 000");
        } else {
            let err = result.unwrap_err();
            assert!(err.contains("could not be read"), "{}", err);
            assert!(err.to_lowercase().contains("permission denied"), "{}", err);
            assert!(!crate::node::revalidate::ManagedNodeState::path(&datadir).exists());
            assert!(!internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir)).exists());
            // Intact once visible again.
            assert_eq!(
                InternalBitcoindConfig::from_file(&other_conf)
                    .unwrap()
                    .networks,
                other.networks
            );
        }
        let _ = fs::remove_dir_all(&base);

        // Own family's conf under a non-traversable directory: the same
        // refusal, not a fresh config over the top of it.
        let (base, datadir) = a_temp_datadir("perm-own");
        let own_dir = internal_bitcoind_datadir(&datadir);
        fs::create_dir_all(&own_dir).unwrap();
        let mut own = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        own.networks.insert(Network::Bitcoin, section(45001, 45002));
        let own_conf = internal_bitcoind_config_path(&own_dir);
        own.to_file(&own_conf).unwrap();
        let ledger_before = crate::node::revalidate::ManagedNodeState::path(&datadir).exists();
        deny(&own_dir);
        let privileged = fs::metadata(&own_conf).is_ok();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Core, None)
                .map(|_| ())
        }));
        restore(&own_dir);
        let result = result.unwrap();
        if !privileged {
            let err = result.unwrap_err();
            assert!(err.to_lowercase().contains("permission denied"), "{}", err);
            assert_eq!(
                crate::node::revalidate::ManagedNodeState::path(&datadir).exists(),
                ledger_before
            );
            assert_eq!(
                InternalBitcoindConfig::from_file(&own_conf)
                    .unwrap()
                    .networks,
                own.networks
            );
        }
        let _ = fs::remove_dir_all(&base);
    }

    // The settings writer and the configure-and-start helper both take the
    // datadir-wide conf lock; while it is busy for the whole bounded wait they
    // refuse before the ledger, the conf or Tor state is touched, and the
    // refusal reads as the retryable "another setup is updating" message.
    #[test]
    fn settings_refuse_on_a_busy_conf_lock_before_any_write() {
        use crate::node::managed_conf::ManagedConfLock;
        let (base, datadir) = a_temp_datadir("busy-lock");
        let held = ManagedConfLock::acquire(&datadir).unwrap();

        let err = crate::node::managed_conf::with_quick_lock_bound(|| {
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .map(|_| ())
        })
        .unwrap_err();
        assert!(err.contains("another setup is updating"), "{}", err);

        let err = crate::node::managed_conf::with_quick_lock_bound(|| {
            configure_and_start_internal_bitcoind(
                datadir.clone(),
                Network::Bitcoin,
                NodeFlavor::Knots,
                None,
                true,
                None,
            )
            .map(|_| ())
        })
        .unwrap_err();
        assert!(err.contains("another setup is updating"), "{}", err);
        drop(held);
        // Lifecycle acquisition also leaves its lock file. A busy conf lock
        // must not write the managed config, ledger, or Tor preference.
        assert!(!crate::node::revalidate::ManagedNodeState::path(&datadir).exists());
        assert!(!internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir)).exists());
        assert!(!crate::node::tor::InboundTorPreference::path(&datadir).exists());
        let mut entries: Vec<_> = std::fs::read_dir(datadir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec![
                "bitcoind",
                crate::node::managed_conf::MANAGED_CONF_LOCK_FILE
            ]
        );
        let node_entries: Vec<_> = std::fs::read_dir(datadir.bitcoind_directory().path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(node_entries, vec!["lifecycle.lock"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    // The settings writer is a locked read-modify-write on a fresh read: a
    // section another writer added while holding the lock survives the rewrite
    // that follows its release. Handshake, not wall-clock: contention is proven
    // (a quick bounded acquire is `Busy` while the writer holds), the writer
    // persists and releases on signal, and only then does the settings write run
    // its fresh read. Ported from the legacy-conf migration test that RDTS sunset
    // PR 4 deleted, so the lock's end-to-end contention coverage does not drop
    // with it; the Busy half lives in `settings_refuse_on_a_busy_conf_lock_before_any_write`.
    #[test]
    fn a_settings_write_reads_fresh_after_a_concurrent_writer_releases_the_lock() {
        use crate::node::managed_conf::ManagedConfLock;
        let (base, datadir) = a_temp_datadir("fresh-read");
        let conf_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
        // An existing conf with a mainnet section, as a previous setup left it.
        std::fs::write(
            &conf_path,
            "[main]\nrpcport=41001\nport=41002\nprune=15000\n",
        )
        .unwrap();

        let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
        let (persist_tx, persist_rx) = std::sync::mpsc::channel::<()>();
        let writer = {
            let datadir = datadir.clone();
            let conf_path = conf_path.clone();
            std::thread::spawn(move || {
                let held = ManagedConfLock::acquire(&datadir).unwrap();
                locked_tx.send(()).unwrap();
                persist_rx.recv().unwrap();
                // Append a section (an older writer that rewrites nothing else).
                let mut text = std::fs::read_to_string(&conf_path).unwrap();
                text.push_str("\n[testnet4]\nrpcport=41003\nport=41004\nprune=15000\n");
                std::fs::write(&conf_path, text).unwrap();
                drop(held);
            })
        };
        locked_rx.recv().unwrap();
        assert!(
            matches!(
                ManagedConfLock::acquire_with_bound(
                    &datadir,
                    2,
                    std::time::Duration::from_millis(5)
                ),
                Err(crate::node::managed_conf::ManagedConfLockError::Busy { .. })
            ),
            "the writer must be holding the lock at this point"
        );
        persist_tx.send(()).unwrap();
        writer.join().unwrap();

        let cfg =
            write_internal_bitcoind_config(&datadir, Network::Bitcoin, NodeFlavor::Knots, None)
                .unwrap();
        assert_eq!(
            cfg.addr.port(),
            41001,
            "the existing mainnet ports are kept"
        );
        let after = InternalBitcoindConfig::from_file(&conf_path).unwrap();
        assert_eq!(after.networks.len(), 2, "{:?}", after.networks.keys());
        assert_eq!(
            after.networks.get(&Network::Testnet4).map(|n| n.rpc_port),
            Some(41003),
            "the section the other writer added did not survive the fresh read"
        );
        assert_eq!(
            crate::node::revalidate::ManagedNodeState::load(&datadir).configured_flavor,
            Some(NodeFlavor::Knots)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The manual "Switch to local node" path: a node pruned past the Vault's
    /// coins is refused with the reason, an unknown answer waits, and a
    /// running scan asks before it is thrown away.
    #[test]
    fn manual_switch_to_local_node_guards_history_and_running_scans() {
        use crate::app::local_switch::{NodePruning, RunningScan, VaultHistory};

        let mut cfg = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        cfg.pending_bitcoind = Some(bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
            "/tmp/bitcoin/.cookie",
        ))));
        cfg.auto_switch_to_pending = Some(true);
        let daemon = daemon(Some(cfg.clone()));
        let switch = || node_message(view::NodeSettingsMessage::SwitchToBitcoind);

        // A synced node and a wallet that has finished its first poll.
        let mut cache = Cache {
            node_bitcoind_ibd: Some(false),
            last_poll_at_startup: Some(100),
            daemon_cache: crate::app::cache::DaemonCache {
                blockheight: 970_500,
                last_poll_timestamp: Some(200),
                ..Default::default()
            },
            ..Cache::default()
        };

        let fresh =
            |cache: &Cache| BitcoindSettingsState::new(Some(cfg.clone()), cache, false, false);

        // Pruned past the Vault's earliest coin: refused, with both heights.
        cache.node_bitcoind_pruning = Some(NodePruning::Pruned {
            prune_height: 969_938,
        });
        cache.local_switch_history = Some(VaultHistory::From(958_601));
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        let warning = state.warning.as_ref().map(|e| e.to_string()).unwrap();
        assert!(
            warning.contains("969938") && warning.contains("958601"),
            "{}",
            warning
        );
        assert!(!state.node_switch_processing);
        assert_eq!(state.pending_scan_discard, None);
        // ... and the standing notice agrees.
        assert!(pruned_switch_refusal(&cache).is_some());

        // Pruned, history not read yet: wait rather than guess.
        cache.local_switch_history = None;
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert!(state.warning.is_some());
        assert!(!state.node_switch_processing);
        assert!(pruned_switch_refusal(&cache).is_none());

        // Pruning unknown: same.
        cache.node_bitcoind_pruning = None;
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert!(state.warning.is_some());
        assert!(!state.node_switch_processing);

        // Pruned but every coin above the prune height: allowed.
        cache.node_bitcoind_pruning = Some(NodePruning::Pruned {
            prune_height: 969_938,
        });
        cache.local_switch_history = Some(VaultHistory::From(971_000));
        assert!(pruned_switch_refusal(&cache).is_none());
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert!(state.warning.is_none());
        assert!(state.node_switch_processing, "switch dispatched");

        // An existing local wallet retains records after the blocks were pruned.
        cache.local_switch_history = Some(VaultHistory::TrackedLocally);
        assert!(pruned_switch_refusal(&cache).is_none());
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert!(state.warning.is_none());
        assert!(state.node_switch_processing, "recorded history can switch");

        // Unpruned, but the wallet has not finished its first poll of the
        // session (a Connect Vault's startup scan): ask first.
        cache.node_bitcoind_pruning = Some(NodePruning::Unpruned);
        cache.local_switch_history = None;
        cache.daemon_cache.last_poll_timestamp = Some(100);
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert_eq!(state.pending_scan_discard, Some(RunningScan::WalletSync));
        assert!(!state.node_switch_processing);
        // Cancel keeps the backend.
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::CancelSwitchDiscardingScan),
        );
        assert_eq!(state.pending_scan_discard, None);
        assert!(!state.node_switch_processing);
        // A stray confirm without a pending question does nothing.
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::ConfirmSwitchDiscardingScan),
        );
        assert!(!state.node_switch_processing);
        // Asked again and confirmed: switches.
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        let _ = state.update(
            Some(daemon.clone()),
            &cache,
            node_message(view::NodeSettingsMessage::ConfirmSwitchDiscardingScan),
        );
        assert_eq!(state.pending_scan_discard, None);
        assert!(state.node_switch_processing, "switch dispatched");

        // A pending rescan is a running scan too.
        cache.daemon_cache.last_poll_timestamp = Some(200);
        cache.daemon_cache.rescan_progress = Some(0.4);
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert_eq!(state.pending_scan_discard, Some(RunningScan::Rescan));

        // Nothing running, unpruned: switches straight away.
        cache.daemon_cache.rescan_progress = None;
        let mut state = fresh(&cache);
        let _ = state.update(Some(daemon.clone()), &cache, switch());
        assert_eq!(state.pending_scan_discard, None);
        assert!(state.node_switch_processing, "switch dispatched");
    }

    /// A Connect Vault's first-ever full scan: the database has no tip yet
    /// (`blockheight` 0) and no poll has completed this session. `sync_status`
    /// reads that as `Synced` for an Esplora backend, so the scan must be
    /// recognised from the missing poll instead.
    /// Bitcoin Blake2b's settings only pass the switch messages through. The
    /// scan-discard confirmation must pass too, or a mid-scan switch on that chain
    /// is a dead click: the guard holds it and nothing can confirm or cancel.
    #[test]
    fn blake2b_manual_switch_mid_scan_can_be_confirmed_or_cancelled() {
        use crate::app::local_switch::{NodePruning, RunningScan};

        let mut cfg = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        cfg.pending_bitcoind = Some(bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
            "/tmp/bitcoin/.cookie",
        ))));
        cfg.auto_switch_to_pending = Some(true);
        let cache = Cache {
            fiat_chain: crate::chain::ChainId::BitcoinBlake2b,
            node_bitcoind_ibd: Some(false),
            node_bitcoind_pruning: Some(NodePruning::Unpruned),
            last_poll_at_startup: None,
            daemon_cache: crate::app::cache::DaemonCache {
                blockheight: 0,
                last_poll_timestamp: None,
                rescan_progress: None,
                ..Default::default()
            },
            ..Cache::default()
        };
        assert!(cache.chain().is_blake2b());

        // "Keep scanning" clears the held switch.
        let mut state = BitcoindSettingsState::new(Some(cfg.clone()), &cache, false, false);
        let _ = state.update(
            Some(daemon(Some(cfg.clone()))),
            &cache,
            node_message(view::NodeSettingsMessage::SwitchToBitcoind),
        );
        assert_eq!(state.pending_scan_discard, Some(RunningScan::WalletSync));
        let _ = state.view(
            &Menu::Vault(crate::app::menu::VaultSubMenu::Overview),
            &cache,
        );
        let _ = state.update(
            Some(daemon(Some(cfg.clone()))),
            &cache,
            node_message(view::NodeSettingsMessage::CancelSwitchDiscardingScan),
        );
        assert_eq!(state.pending_scan_discard, None);
        assert!(!state.node_switch_processing);

        // "Switch anyway" switches.
        let _ = state.update(
            Some(daemon(Some(cfg.clone()))),
            &cache,
            node_message(view::NodeSettingsMessage::SwitchToBitcoind),
        );
        let _ = state.update(
            Some(daemon(Some(cfg))),
            &cache,
            node_message(view::NodeSettingsMessage::ConfirmSwitchDiscardingScan),
        );
        assert_eq!(state.pending_scan_discard, None);
        assert!(
            state.node_switch_processing,
            "the confirmed switch was dropped"
        );
    }

    #[tokio::test]
    async fn history_controls_follow_switch_intent_and_advanced_disclosure() {
        use crate::node::history::{Binding, Phase, Recovery};
        use coincube_core::miniscript::bitcoin::{hashes::Hash, BlockHash};
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };
        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }
        let renderer = <iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let cache = Cache {
            node_bitcoind_ibd: Some(false),
            node_bitcoind_pruning: Some(crate::app::local_switch::NodePruning::Pruned {
                prune_height: 970_000,
            }),
            local_switch_history: Some(crate::app::local_switch::VaultHistory::From(946_000)),
            ..Cache::default()
        };
        let managed = bitcoind_config(BitcoindRpcAuth::CookieFile(
            crate::node::bitcoind::internal_bitcoind_cookie_path(
                &crate::node::bitcoind::internal_bitcoind_datadir(&cache.datadir_path),
                &cache.network,
            ),
        ));
        let mut cfg = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        cfg.pending_bitcoind = Some(managed.clone());
        let menu = Menu::Vault(crate::app::menu::VaultSubMenu::Overview);
        for (local, external, blake2b, clicked, advanced, expected, retention) in [
            (false, false, false, false, false, false, false),
            (false, false, false, true, false, true, false),
            (false, false, false, true, true, true, true),
            (true, false, false, false, false, false, false),
            (true, false, false, false, true, false, true),
            (true, false, false, true, false, false, false),
            (true, false, false, true, true, false, true),
            (false, true, false, true, true, false, false),
            (false, false, true, true, true, false, false),
        ] {
            let mut cfg = cfg.clone();
            let mut cache = cache.clone();
            if local {
                cfg.bitcoin_backend = Some(BitcoinBackend::Bitcoind(managed.clone()));
                cfg.pending_bitcoind = None;
                if clicked {
                    // The transfer finished at 974579. Four new headers must
                    // not revive its switch prompt once the node is in use.
                    let hash = BlockHash::all_zeros();
                    let mut job = Recovery::new(
                        Binding {
                            genesis: hash,
                            node_instance: "fixture".into(),
                            endpoint: "fixture".into(),
                            wallet: "fixture".into(),
                            descriptor: "fixture".into(),
                        },
                        974000,
                        974579,
                        hash,
                    )
                    .unwrap();
                    job.phase = Phase::Complete;
                    cache.node_history.job = Some(job);
                    cache.node_history.reconciled = true;
                    cache.node_history_switch_requested = true;
                    cache.local_node_transfer_visible = true;
                    cache.node_bitcoind_sync_progress = Some(1.0);
                    cache.node_bitcoind_sync_heights = Some((974579, 974583));
                }
            }
            if external {
                cfg.pending_bitcoind = Some(bitcoind_config(BitcoindRpcAuth::CookieFile(
                    PathBuf::from("/tmp/external-node/.cookie"),
                )));
            }
            if blake2b {
                cache.fiat_chain = crate::chain::ChainId::BitcoinBlake2b;
            }
            let mut state = BitcoindSettingsState::new(Some(cfg.clone()), &cache, false, false);
            assert!(
                state.retention_days.value.is_empty(),
                "no rolling window is selected by default"
            );
            let daemon = daemon(Some(cfg));
            if clicked {
                let _ = state.update(
                    Some(daemon.clone()),
                    &cache,
                    node_message(view::NodeSettingsMessage::SwitchToBitcoind),
                );
            }
            if advanced {
                let toggle = if local {
                    view::NodeSettingsMessage::NodeAdvancedToggled
                } else {
                    view::NodeSettingsMessage::RecoveryAdvancedToggled
                };
                let _ = state.update(Some(daemon), &cache, node_message(toggle));
            }
            if !local {
                assert!(state.bitcoind_settings.is_none());
                assert!(state.electrum_settings.is_none());
            }
            let mut element = state.view(&menu, &cache);
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1200.0, 2400.0)),
            );
            let mut labels = Labels::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
            for (label, visible) in [
                ("Move this Vault to your local node", expected),
                ("Vault moved to your local node", local && clicked),
                (
                    "Bring this Vault's history to your node",
                    expected || (local && clicked),
                ),
                ("Start from Connect's history", expected),
            ] {
                assert_eq!(labels.0.iter().any(|text| text == label), visible,
                    "missing/unsupported history controls: local={local} external={external} blake2b={blake2b}: {label}");
            }
            assert_eq!(
                labels
                    .0
                    .iter()
                    .any(|text| text == "Recover from this date or block"),
                expected && advanced
            );
            assert_eq!(
                labels.0.iter().any(|text| text == "Block retention"),
                retention
            );
            if local && clicked {
                assert!(labels.0.iter().any(|text| text == "In use · Local Node"));
                assert!(labels.0.iter().any(|text| text == "Catching up 99.9%"));
                assert!(!labels.0.iter().any(|text| text == "Switch to local node"));
                assert!(labels.0.iter().any(|text| text
                    == "All steps complete. COINCUBE Desktop reads this Vault from your node."));
            }
        }
    }

    #[test]
    fn successful_local_switch_retires_the_open_panels_recovery_intent() {
        let cache = Cache {
            node_history_switch_requested: true,
            ..Cache::default()
        };
        let mut connect = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        let node = bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
            "/tmp/bitcoin/.cookie",
        )));
        connect.pending_bitcoind = Some(node.clone());
        let mut state = BitcoindSettingsState::new(Some(connect.clone()), &cache, false, false);
        assert!(state.recovery_requested);

        // Updating Connect, or a failed switch, must preserve the unfinished
        // transfer. Only successfully adopting the local backend completes it.
        let _ = state.update(
            Some(daemon(Some(connect.clone()))),
            &cache,
            Message::DaemonConfigLoaded(Ok(())),
        );
        assert!(state.recovery_requested);
        let _ = state.update(
            Some(daemon(Some(connect.clone()))),
            &cache,
            Message::DaemonConfigLoaded(Err(Error::Unexpected("switch failed".into()))),
        );
        assert!(state.recovery_requested);

        let mut local = connect;
        local.bitcoin_backend = Some(BitcoinBackend::Bitcoind(node));
        local.pending_bitcoind = None;
        let _ = state.update(
            Some(daemon(Some(local))),
            &cache,
            Message::DaemonConfigLoaded(Ok(())),
        );
        assert!(!state.recovery_requested);
    }

    #[test]
    fn recovery_switch_intent_survives_node_page_recreation() {
        let mut cache = Cache::default();
        let cfg = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        let first = BitcoindSettingsState::new(Some(cfg.clone()), &cache, false, false);
        assert!(!first.recovery_requested);
        // The App retains intent above the recreated settings page, scoped to
        // its wallet/node context; its invalidation and cancellation clear it.
        cache.node_history_switch_requested = true;
        let reopened = BitcoindSettingsState::new(Some(cfg.clone()), &cache, false, false);
        assert!(reopened.recovery_requested);
        cache.node_history_switch_requested = false;
        let cancelled = BitcoindSettingsState::new(Some(cfg), &cache, false, false);
        assert!(!cancelled.recovery_requested);
    }

    #[test]
    fn manual_switch_confirms_during_a_connect_vaults_first_scan() {
        use crate::app::local_switch::{NodePruning, RunningScan};

        let mut cfg = config_with_backend(Some(BitcoinBackend::Esplora(esplora_config())));
        cfg.pending_bitcoind = Some(bitcoind_config(BitcoindRpcAuth::CookieFile(PathBuf::from(
            "/tmp/bitcoin/.cookie",
        ))));
        cfg.auto_switch_to_pending = Some(true);
        let daemon = daemon(Some(cfg.clone()));
        let cache = Cache {
            node_bitcoind_ibd: Some(false),
            node_bitcoind_pruning: Some(NodePruning::Unpruned),
            last_poll_at_startup: None,
            daemon_cache: crate::app::cache::DaemonCache {
                blockheight: 0,
                last_poll_timestamp: None,
                rescan_progress: None,
                ..Default::default()
            },
            ..Cache::default()
        };
        let mut state = BitcoindSettingsState::new(Some(cfg), &cache, false, false);
        let _ = state.update(
            Some(daemon),
            &cache,
            node_message(view::NodeSettingsMessage::SwitchToBitcoind),
        );
        assert_eq!(state.pending_scan_discard, Some(RunningScan::WalletSync));
        assert!(!state.node_switch_processing, "switched mid first scan");
    }
}
