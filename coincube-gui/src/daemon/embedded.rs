use coincubed::bip329::Labels;
use coincubed::commands::{CommandError, UpdateDerivIndexesResult};
use std::collections::{HashMap, HashSet};
use tokio::sync::Mutex;

use super::{model::*, node, Daemon, DaemonBackend, DaemonError};
use crate::dir::CoincubeDirectory;
use async_trait::async_trait;
use coincube_core::miniscript::bitcoin::{
    address, bip32::ChildNumber, psbt::Psbt, Address, Network, OutPoint, Txid,
};
use coincubed::{
    commands::{CoinStatus, LabelItem},
    config::Config,
    DaemonControl, DaemonHandle,
};

fn authenticated_startup_error(error: coincubed::StartupError) -> DaemonError {
    match error {
        coincubed::StartupError::ConnectAdmission(error) => {
            DaemonError::ConnectAnchor(error.into())
        }
        error => DaemonError::Start(error),
    }
}

fn map_spend_command_error(error: CommandError) -> DaemonError {
    match error {
        CommandError::UnsafeLegacyAlternative {
            input,
            legacy_signatures,
        } => DaemonError::UnsafeLegacyAlternative(
            CommandError::UnsafeLegacyAlternative {
                input,
                legacy_signatures,
            }
            .to_string(),
        ),
        // Reported exactly as an external daemon reports it over JSON-RPC, so a
        // refused broadcast reads the same whichever daemon the GUI talks to.
        error @ CommandError::TxBroadcast(_) => DaemonError::Rpc(
            coincubed::commands::TX_BROADCAST_ERROR as i32,
            error.to_string(),
        ),
        CommandError::FeeCeilingExceeded => DaemonError::FeeCeilingExceeded,
        CommandError::FeeRateExceeded => DaemonError::FeeRateExceeded,
        other => DaemonError::Unexpected(other.to_string()),
    }
}

// A Cube locked moments ago may still be stopping its daemon in the
// background, holding this directory. Refuse rather than start a second
// daemon on it: Windows has no `daemon.lock` to stop that.
fn wait_for_previous_daemon(config: &Config) -> Result<(), DaemonError> {
    wait_for_previous_daemon_within(config, super::stopping::WAIT_LIMIT)
}

fn wait_for_previous_daemon_within(
    config: &Config,
    limit: std::time::Duration,
) -> Result<(), DaemonError> {
    match config.data_directory() {
        Some(dir) if !super::stopping::wait_for_within(dir.path(), limit) => Err(
            DaemonError::Unexpected(super::stopping::STILL_STOPPING.to_string()),
        ),
        _ => Ok(()),
    }
}

// If an async caller abandons startup, the blocking result still owns cleanup.
// DaemonHandle itself does not stop its poller on drop.
struct PendingConnectDaemon(Option<DaemonHandle>);
impl Drop for PendingConnectDaemon {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            if let Err(error) = handle.stop_for_cleanup() {
                log::error!("Fork daemon cleanup failed: {}", error);
            }
        }
    }
}

pub struct EmbeddedDaemon {
    config: Config,
    handle: std::sync::Arc<Mutex<Option<DaemonHandle>>>,
    connect_session:
        Option<std::sync::Arc<crate::services::coincube::network_anchor::ConnectAnchorSession>>,
}

impl EmbeddedDaemon {
    /// GUI construction tests inspect startup handoff without starting a poller.
    #[cfg(test)]
    pub(crate) fn unstarted_for_test(
        config: Config,
        connect_session: Option<
            std::sync::Arc<crate::services::coincube::network_anchor::ConnectAnchorSession>,
        >,
    ) -> Self {
        Self {
            config,
            handle: std::sync::Arc::new(Mutex::new(None)),
            connect_session,
        }
    }

    /// Tests can supply a disposable daemon handle without a production backend.
    #[cfg(test)]
    pub(crate) fn from_regtest_handle(config: Config, handle: DaemonHandle) -> Self {
        Self {
            config,
            handle: std::sync::Arc::new(Mutex::new(Some(handle))),
            connect_session: None,
        }
    }

    pub fn start(config: Config) -> Result<EmbeddedDaemon, DaemonError> {
        wait_for_previous_daemon(&config)?;
        let handle =
            DaemonHandle::start_default(config.clone(), false).map_err(DaemonError::Start)?;
        Ok(Self {
            handle: std::sync::Arc::new(Mutex::new(Some(handle))),
            config,
            connect_session: None,
        })
    }

    /// Local fork startup trusts the selected consensus-validating node and
    /// requires its exact-chain RPC admission before wallet/database writes.
    pub fn start_local_fork(config: Config) -> Result<Self, DaemonError> {
        wait_for_previous_daemon(&config)?;
        let handle =
            DaemonHandle::start_with_local_node(config.clone()).map_err(DaemonError::Start)?;
        Ok(Self {
            config,
            handle: std::sync::Arc::new(Mutex::new(Some(handle))),
            connect_session: None,
        })
    }

    /// Only explicit authenticated Connect authority can admit a fork daemon.
    /// The retained configuration is safe for existing UI persistence callers.
    pub async fn start_authenticated(
        mut config: Config,
        client: crate::services::coincube::CoincubeClient,
    ) -> Result<Self, DaemonError> {
        let chain = config.bitcoin_config.chain;
        if !chain.is_blake2b() {
            return Err(DaemonError::ConnectAnchor(
                coincubed::connect::AdmissionError::InvalidBackend.into(),
            ));
        }
        if let Some(pending) = &config.pending_bitcoind {
            let managed = config
                .data_directory()
                .and_then(|dir| {
                    dir.path().ancestors().nth(3).map(|root| {
                        let expected = crate::node::bitcoind::internal_bitcoind_datadir_for(
                            &CoincubeDirectory::new(root.to_path_buf()),
                            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
                        );
                        pending.addr.ip().is_loopback()
                            && pending.rpc_auth
                                == coincubed::config::BitcoindRpcAuth::CookieFile(
                                    crate::node::bitcoind::internal_bitcoind_cookie_path(
                                        &expected,
                                        &chain.bitcoin_network(),
                                    ),
                                )
                    })
                })
                .unwrap_or(false);
            if !managed {
                return Err(DaemonError::ConnectAnchor(
                    coincubed::connect::AdmissionError::InvalidBackend.into(),
                ));
            }
        }
        let endpoint = match &config.bitcoin_backend {
            Some(coincubed::config::BitcoinBackend::Esplora(esplora))
                if esplora.fallback_addr.is_none() && esplora.secondary_fallback_addr.is_none() =>
            {
                esplora.addr.clone()
            }
            _ => {
                return Err(DaemonError::ConnectAnchor(
                    coincubed::connect::AdmissionError::InvalidBackend.into(),
                ))
            }
        };
        let (backend, session) = client
            .authenticated_backend(chain, &endpoint)
            .await
            .map_err(DaemonError::ConnectAnchor)?;
        let retained_config = config.for_persistence();
        // The GUI owns the syncing companion; the authenticated daemon retains
        // its narrow Connect-only capability and never starts a pending node.
        config.pending_bitcoind = None;
        if let Some(dir) = config.data_directory() {
            if !super::stopping::wait_for_async(dir.path().to_path_buf()).await {
                session.invalidate();
                return Err(DaemonError::Unexpected(
                    super::stopping::STILL_STOPPING.to_string(),
                ));
            }
        }
        let result = tokio::task::spawn_blocking(move || {
            DaemonHandle::start_with_connect(config, backend, false)
                .map(|handle| PendingConnectDaemon(Some(handle)))
        })
        .await
        .map_err(|_| DaemonError::Unexpected("Connect daemon startup task failed".to_string()))?;
        match result {
            Ok(mut pending) => Ok(Self {
                config: retained_config,
                handle: std::sync::Arc::new(Mutex::new(pending.0.take())),
                connect_session: Some(session),
            }),
            Err(error) => {
                session.invalidate();
                Err(authenticated_startup_error(error))
            }
        }
    }

    #[cfg(feature = "integration-tests")]
    pub async fn integration_rescan_time_bounds(
        &self,
    ) -> Result<coincubed::commands::RescanTimeBounds, DaemonError> {
        self.command(|daemon| {
            daemon
                .rescan_time_bounds()
                .map_err(|error| DaemonError::Unexpected(error.to_string()))
        })
        .await
    }

    pub async fn command<T, F>(&self, method: F) -> Result<T, DaemonError>
    where
        F: FnOnce(&mut DaemonControl) -> Result<T, DaemonError>,
    {
        match self.handle.lock().await.as_mut() {
            Some(DaemonHandle::Controller { control, .. }) => method(control),
            Some(_) => unreachable!("No coincubed rpc server must be started"),
            None => Err(DaemonError::DaemonStopped),
        }
    }
}

impl Drop for EmbeddedDaemon {
    fn drop(&mut self) {
        // Existing Bitcoin lifecycle is unchanged. A canceled fork startup or
        // abandoned GUI owner must not retain a poller and its authentication.
        if self.config.bitcoin_config.chain.is_blake2b() {
            if let Some(session) = &self.connect_session {
                session.invalidate();
            }
            let cleanup = |handle: Option<DaemonHandle>| {
                if let Some(handle) = handle {
                    if let Err(error) = handle.stop_for_cleanup() {
                        log::error!("Fork daemon cleanup failed: {}", error);
                    }
                }
            };
            if let Some(handle) = std::sync::Arc::get_mut(&mut self.handle) {
                cleanup(handle.get_mut().take());
            } else {
                // Another holder may still own the lifecycle guard. Its
                // cleanup must wait off the async executor for that holder.
                let handle = self.handle.clone();
                std::thread::spawn(move || {
                    let pending = handle.blocking_lock().take();
                    cleanup(pending);
                });
            }
        }
    }
}

impl<T> From<std::sync::PoisonError<T>> for DaemonError {
    fn from(value: std::sync::PoisonError<T>) -> Self {
        DaemonError::Unexpected(format!("Daemon panic: {}", value))
    }
}

impl std::fmt::Debug for EmbeddedDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle").finish()
    }
}

#[async_trait]
impl Daemon for EmbeddedDaemon {
    fn backend(&self) -> DaemonBackend {
        let node_type = self
            .config
            .bitcoin_backend
            .as_ref()
            .map(node::NodeType::from);
        DaemonBackend::EmbeddedCoincubed(node_type)
    }

    fn config(&self) -> Option<&Config> {
        Some(&self.config)
    }

    fn invalidate_connect_session(&self) {
        if let Some(session) = &self.connect_session {
            session.invalidate();
        }
    }

    async fn is_alive(
        &self,
        _datadir: &CoincubeDirectory,
        _network: Network,
    ) -> Result<(), DaemonError> {
        let mut handle = self.handle.lock().await;
        if let Some(h) = handle.as_ref() {
            if h.is_alive() {
                return Ok(());
            }
        }
        // Revoke the authenticated refresh task before handling a dead poller.
        self.invalidate_connect_session();
        if let Some(h) = handle.take() {
            if self.connect_session.is_some() {
                h.stop_for_cleanup()
                    .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
            } else {
                h.stop()
                    .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
            }
        }
        if self.connect_session.is_some() {
            return Err(DaemonError::DaemonStopped);
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), DaemonError> {
        self.invalidate_connect_session();
        let mut handle = self.handle.lock().await;
        if let Some(h) = handle.take() {
            if self.connect_session.is_some() {
                h.stop_for_cleanup()
                    .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
            } else {
                h.stop()
                    .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
            }
        }
        Ok(())
    }

    async fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        self.command(|daemon| Ok(daemon.get_info())).await
    }

    async fn chain_tip_hash(
        &self,
    ) -> Result<coincube_core::miniscript::bitcoin::BlockHash, DaemonError> {
        self.command(|daemon| daemon.chain_tip_hash().ok_or(DaemonError::NotImplemented))
            .await
    }

    async fn request_sync(&self) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon.request_sync();
            Ok(())
        })
        .await
    }

    async fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .try_get_new_address()
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .list_revealed_addresses(is_change, exclude_used, limit, start_index)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .update_deriv_indexes(receive, change)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<ListCoinsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_coins(statuses, outpoints)))
            .await
    }

    async fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .list_spend(None)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn list_confirmed_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_confirmed_transactions(start, end, limit)))
            .await
    }

    async fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_transactions(txids)))
            .await
    }

    async fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .create_spend(destinations, coins_outpoints, feerate_vb, change_address)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn create_spend_tx_capped(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
        max_fee_sats: u64,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .create_spend_within_limits(
                    destinations,
                    coins_outpoints,
                    feerate_vb,
                    change_address,
                    Some(max_fee_sats),
                )
                .map_err(|error| match error {
                    coincubed::commands::CommandError::FeeCeilingExceeded => {
                        DaemonError::FeeCeilingExceeded
                    }
                    coincubed::commands::CommandError::FeeRateExceeded => {
                        DaemonError::FeeRateExceeded
                    }
                    other => DaemonError::Unexpected(other.to_string()),
                })
        })
        .await
    }

    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .rbf_psbt(txid, is_cancel, feerate_vb)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .update_spend(psbt.clone())
                .map_err(map_spend_command_error)
        })
        .await
    }

    async fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon.delete_spend(txid);
            Ok(())
        })
        .await
    }

    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .broadcast_spend(txid)
                .map_err(map_spend_command_error)
        })
        .await
    }

    async fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .start_rescan(t)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn replay_wallet_records(&self) -> Result<(), DaemonError> {
        let mut control = match self.handle.lock().await.as_ref() {
            Some(DaemonHandle::Controller { control, .. }) => control.clone(),
            Some(_) => return Err(DaemonError::ClientNotSupported),
            None => return Err(DaemonError::DaemonStopped),
        };
        // Only the control clone enters the worker. Reads and shutdown must
        // remain possible while this synchronous poll acknowledgement waits.
        tokio::task::spawn_blocking(move || {
            control
                .replay_wallet_records()
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
        .map_err(|e| DaemonError::Unexpected(format!("Wallet replay worker failed: {e}")))?
    }

    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        coins_outpoints: &[OutPoint],
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        self.command(|daemon| {
            daemon
                .create_recovery(address, coins_outpoints, feerate_vb, sequence)
                .map(|res| res.psbt)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn get_labels(
        &self,
        items: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        self.command(|daemon| Ok(daemon.get_labels(items).labels))
            .await
    }

    async fn update_labels(
        &self,
        items: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon.update_labels(items);
            Ok(())
        })
        .await
    }

    async fn get_labels_bip329(&self, offset: u32, limit: u32) -> Result<Labels, DaemonError> {
        self.command(|daemon| Ok(daemon.get_labels_bip329(offset, limit).labels))
            .await
    }
}

#[cfg(test)]
mod anchor_startup_tests {
    use super::*;
    use crate::services::coincube::{
        network_anchor::{AnchorStartupError, AnchorState},
        CoincubeClient,
    };
    use coincubed::connect::AdmissionError;
    use httpmock::prelude::*;
    use serde_json::json;

    // Synthetic public descriptor; these refusal tests never reach daemon setup.
    const DESCRIPTOR: &str = "tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c";
    fn config(endpoint: &str, dir: &std::path::Path) -> Config {
        toml::from_str(&format!("main_descriptor = \"{}\"\ndata_directory = '{}'\n[bitcoin_config]\nnetwork = 'bitcoin-blake2b'\n[esplora_config]\naddr = '{}'\n", DESCRIPTOR, dir.display(), endpoint)).unwrap()
    }

    /// A start on a directory whose daemon is still stopping after a lock is
    /// refused, not started beside it: Windows has no `daemon.lock` to stop a
    /// second daemon.
    #[test]
    fn a_start_refuses_while_the_previous_daemon_is_still_stopping() {
        let dir = std::env::temp_dir().join(format!("start-refuse-{}", uuid::Uuid::new_v4()));
        let config = config("http://127.0.0.1:9", &dir);
        let (release, released) = std::sync::mpsc::channel::<()>();
        crate::daemon::stopping::run_in_background(Some(dir.clone()), move || {
            released.recv().unwrap();
        });
        match wait_for_previous_daemon_within(&config, std::time::Duration::from_millis(50)) {
            Err(DaemonError::Unexpected(message)) => {
                assert_eq!(message, crate::daemon::stopping::STILL_STOPPING)
            }
            other => panic!("{}", format!("expected a refusal, got {:?}", other.err())),
        }
        release.send(()).unwrap();
        assert!(
            wait_for_previous_daemon_within(&config, std::time::Duration::from_secs(5)).is_ok()
        );
    }
    fn client(server: &MockServer) -> CoincubeClient {
        let mut client = CoincubeClient::new();
        client.base_url = server.base_url();
        client.set_token("synthetic-anchor-token");
        client
    }
    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("coincube-anchor-refusal-{}", uuid::Uuid::new_v4()))
    }
    #[tokio::test]
    async fn actual_embedded_startup_preserves_each_service_state_without_writes() {
        for state in [
            "not_configured",
            "configuration_error",
            "rpc_unavailable",
            "malformed",
            "fork_absent",
            "fork_inactive",
            "rdts_absent",
            "rdts_unsupported",
            "wrong_chain",
            "syncing",
            "inconsistent_snapshot",
            "fork_unverified",
        ] {
            let server = MockServer::start_async().await;
            server.mock_async(|when,then|{when.method(GET);then.status(503).json_body(json!({"success":false,"error":{"code":"SERVICE_UNAVAILABLE"},"data":{"network":"bitcoin-blake2b","state":state}}));}).await;
            let dir = temp_path();
            let cfg = config(
                &format!(
                    "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
                    server.base_url()
                ),
                &dir,
            );
            let expected = serde_json::from_value::<AnchorState>(json!(state)).unwrap();
            assert!(
                matches!(EmbeddedDaemon::start_authenticated(cfg,client(&server)).await, Err(DaemonError::ConnectAnchor(AnchorStartupError::State(actual))) if actual == expected)
            );
            assert!(!dir.exists());
        }
    }
    #[tokio::test]
    async fn actual_embedded_startup_preserves_http_auth_gate_and_throttle_errors() {
        for code in [401, 403, 404, 429] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(code).body("untrusted upstream text");
                })
                .await;
            let dir = temp_path();
            let cfg = config(
                &format!(
                    "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
                    server.base_url()
                ),
                &dir,
            );
            let result = EmbeddedDaemon::start_authenticated(cfg, client(&server)).await;
            assert!(
                matches!(result, Err(DaemonError::ConnectAnchor(AnchorStartupError::Http(actual))) if actual == code)
            );
            assert!(!dir.exists());
        }
    }
    #[tokio::test]
    async fn startup_keeps_missing_auth_invalid_backend_and_malformed_distinct() {
        let server = MockServer::start_async().await;
        let mock = server.mock_async(|when,then|{when.method(GET);then.status(200).json_body(json!({"success":true,"data":{"network":"bitcoin-blake2b","state":"available"}}));}).await;
        let dir = temp_path();
        let endpoint = format!(
            "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
            server.base_url()
        );
        let mut missing = client(&server);
        missing.clear_token();
        assert!(matches!(
            EmbeddedDaemon::start_authenticated(config(&endpoint, &dir), missing).await,
            Err(DaemonError::ConnectAnchor(AnchorStartupError::Admission(
                AdmissionError::MissingAuth
            )))
        ));
        assert!(matches!(
            EmbeddedDaemon::start_authenticated(
                config("http://127.0.0.1:1/bitcoin", &dir),
                client(&server)
            )
            .await,
            Err(DaemonError::ConnectAnchor(AnchorStartupError::Admission(
                AdmissionError::InvalidBackend
            )))
        ));
        mock.assert_hits_async(0).await;
        assert!(matches!(
            EmbeddedDaemon::start_authenticated(config(&endpoint, &dir), client(&server)).await,
            Err(DaemonError::ConnectAnchor(
                AnchorStartupError::InvalidResponse
            ))
        ));
        mock.assert_hits_async(1).await;
        assert!(!dir.exists());
    }
    #[tokio::test]
    async fn missing_poller_revokes_authority_before_reporting_stopped() {
        use coincubed::connect::ConnectAnchorAuthority;
        use std::time::{SystemTime, UNIX_EPOCH};
        let server = MockServer::start_async().await;
        server.mock_async(|when, then| {
            when.method(GET);
            then.status(200).json_body(json!({"success":true,"data":{
                "network":"bitcoin-blake2b","state":"available","anchor":{
                    "tip_hash":"11".repeat(32),"tip_height":973029,
                    "tip_median_time_past":1800000000,
                    "observed_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                    "observation":{"tip_height":973029,
                        "fork":{"height":972000,"active":true},
                        "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
                }
            }}));
        }).await;
        let endpoint = format!(
            "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
            server.base_url()
        );
        let (_, session) = client(&server)
            .authenticated_backend(coincube_core::chain::ChainId::BitcoinBlake2b, &endpoint)
            .await
            .unwrap();
        assert!(session.fresh_anchor().is_ok());
        let dir = temp_path();
        let daemon = EmbeddedDaemon {
            config: config(&endpoint, &dir),
            handle: std::sync::Arc::new(Mutex::new(None)),
            connect_session: Some(session.clone()),
        };
        let directory = CoincubeDirectory::new(dir.clone());
        assert!(matches!(
            daemon.is_alive(&directory, Network::Bitcoin).await,
            Err(DaemonError::DaemonStopped)
        ));
        assert_eq!(session.fresh_anchor(), Err(AdmissionError::Unavailable));
        drop(daemon);
        assert!(!dir.exists());
    }
    #[tokio::test]
    async fn owner_drop_revokes_authority_while_a_worker_keeps_lifecycle_lock() {
        use coincubed::connect::ConnectAnchorAuthority;
        use std::time::{SystemTime, UNIX_EPOCH};
        let server = MockServer::start_async().await;
        server.mock_async(|when, then| {
            when.method(GET);
            then.status(200).json_body(json!({"success":true,"data":{
                "network":"bitcoin-blake2b","state":"available","anchor":{
                    "tip_hash":"11".repeat(32),"tip_height":973029,
                    "tip_median_time_past":1800000000,
                    "observed_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                    "observation":{"tip_height":973029,
                        "fork":{"height":972000,"active":true},
                        "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
                }
            }}));
        }).await;
        let endpoint = format!(
            "{}/api/v1/esplora/bitcoin-blake2b/mainnet",
            server.base_url()
        );
        let (_, session) = client(&server)
            .authenticated_backend(coincube_core::chain::ChainId::BitcoinBlake2b, &endpoint)
            .await
            .unwrap();
        assert!(session.fresh_anchor().is_ok());
        let dir = temp_path();
        let daemon = EmbeddedDaemon {
            config: config(&endpoint, &dir),
            handle: std::sync::Arc::new(Mutex::new(None)),
            connect_session: Some(session.clone()),
        };
        let handle = daemon.handle.clone();
        let guard = handle.clone().lock_owned().await;
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        // A blocking worker that owns the lifecycle guard, as any holder of
        // a clone of the handle may.
        let worker = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            entered.send(()).unwrap();
            released
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started)
            .await
            .unwrap()
            .unwrap();
        // Exercise the actual owner Drop, not only the blocking helper. It must
        // revoke authentication immediately without waiting on its held lock.
        drop(daemon);
        assert_eq!(session.fresh_anchor(), Err(AdmissionError::Unavailable));
        assert!(handle.try_lock().is_err());
        release.send(()).unwrap();
        worker.await.unwrap();
        // The cleanup thread and worker must both release their ownership after
        // the guarded operation finishes. No poller is installed in this test.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while std::sync::Arc::strong_count(&handle) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(handle.try_lock().unwrap().is_none());
        assert!(!dir.exists());
    }
    #[test]
    fn daemon_admission_results_keep_the_typed_startup_recovery_path() {
        for expected in [
            AdmissionError::MissingAuth,
            AdmissionError::Unavailable,
            AdmissionError::WrongChain,
            AdmissionError::Stale,
            AdmissionError::HashMismatch,
            AdmissionError::ChangedDuringOperation,
            AdmissionError::InvalidBackend,
            AdmissionError::IndexerBehind,
            AdmissionError::Throttled,
            AdmissionError::Aborted,
        ] {
            assert!(matches!(
                authenticated_startup_error(coincubed::StartupError::ConnectAdmission(expected)),
                DaemonError::ConnectAnchor(AnchorStartupError::Admission(actual)) if actual == expected
            ));
        }
    }
}

#[cfg(test)]
mod local_drop_tests {
    use super::*;
    #[test]
    fn abandoned_local_daemon_stops_and_joins_owned_workers_without_connect() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        };
        let path = std::env::temp_dir().join(format!("local-drop-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (config, _) =
            crate::chain::managed_local_fixture(&root, crate::chain::ChainId::BitcoinBlake2b);
        let (sender, receiver) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = stopped.clone();
        let poller = std::thread::spawn(move || {
            assert_eq!(format!("{:?}", receiver.recv().unwrap()), "Shutdown");
            done.store(true, Ordering::SeqCst);
        });
        let abort = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let daemon = EmbeddedDaemon {
            config,
            connect_session: None,
            handle: Arc::new(Mutex::new(Some(DaemonHandle::Server {
                poller_sender: sender,
                poller_handle: poller,
                rpcserver_shutdown: shutdown.clone(),
                rpcserver_handle: std::thread::spawn(|| Ok(())),
                scan_abort: abort.clone(),
                datadir_lock: None,
            }))),
        };
        drop(daemon);
        assert!(stopped.load(Ordering::SeqCst));
        assert!(abort.load(Ordering::SeqCst));
        assert!(shutdown.load(Ordering::SeqCst));
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(test)]
mod spend_command_error_tests {
    use super::*;

    /// A refused broadcast must reach the GUI as the external daemon reports it
    /// over JSON-RPC, not as an unexpected failure: that is what lets the GUI tell
    /// the user why, and whether trying again can help.
    #[test]
    fn a_refused_broadcast_keeps_its_rpc_code() {
        assert!(matches!(
            map_spend_command_error(CommandError::TxBroadcast("insufficient fee".to_string())),
            DaemonError::Rpc(code, message)
                if i64::from(code) == coincubed::commands::TX_BROADCAST_ERROR
                    && message == "Failed to broadcast transaction: insufficient fee"
        ));
    }
}
