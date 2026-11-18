//! Polls local-node synchronization and admits results for the current Vault.
use super::*;

impl App {
    pub(super) fn poll_bitcoind_sync(&mut self) -> Task<Message> {
        if !self.bitcoind_sync_probe_in_progress {
            if let Some(pending_cfg) = self
                .daemon
                .as_ref()
                .and_then(|d| d.config())
                .and_then(local_node_sync_config)
                .cloned()
            {
                self.bitcoind_sync_probe_in_progress = true;
                let app = self.cache.app_generation;
                let history_epoch = self.node_history_epoch;
                let wallet = self.wallet.as_ref().map(|w| w.id());
                // Only a *pending* node is a switch target, so only then
                // is the Vault's history worth reading.
                let history_probe = match (&self.daemon, &self.wallet) {
                    (Some(daemon), Some(wallet))
                        if daemon
                            .config()
                            .is_some_and(|c| c.pending_bitcoind.is_some()) =>
                    {
                        Some(VaultHistoryProbe {
                            daemon: daemon.clone(),
                            data_dir: self.cache.datadir_path.clone(),
                            chain: self.cache.chain(),
                            wallet: wallet.clone(),
                        })
                    }
                    _ => None,
                };
                return Task::perform(
                    async move {
                        let result = check_bitcoind_sync_progress(pending_cfg.clone()).await;
                        // Only a synced, pruned node needs it; an
                        // unpruned one serves any history.
                        let needs_history = result.as_ref().is_ok_and(|s| {
                            !s.ibd
                                && matches!(
                                    s.pruning,
                                    Some(local_switch::NodePruning::Pruned { .. })
                                )
                        });
                        let vault_history = match history_probe {
                            Some(probe) if needs_history => {
                                let pruning = result
                                    .as_ref()
                                    .ok()
                                    .and_then(|s| s.pruning)
                                    .expect("needs_history requires known pruning");
                                probe.read(&pending_cfg, pruning).await
                            }
                            _ => None,
                        };
                        Message::BitcoindSyncProgress {
                            app,
                            history_epoch,
                            wallet,
                            config: pending_cfg,
                            result,
                            vault_history,
                        }
                    },
                    |message| message,
                );
            }
        }
        Task::none()
    }

    pub(super) fn handle_bitcoind_sync_progress(
        &mut self,
        app: cache::AppGeneration,
        history_epoch: u64,
        wallet: Option<WalletId>,
        config: coincubed::config::BitcoindConfig,
        result: Result<LocalNodeSync, String>,
        vault_history: Option<local_switch::VaultHistory>,
    ) -> Task<Message> {
        // Wallet-history permission belongs to the App that asked.
        // A delayed result must not authorize another Vault using the
        // same managed node, or release its in-flight probe guard.
        if app != self.cache.app_generation || wallet != self.wallet.as_ref().map(|w| w.id()) {
            return Task::none();
        }
        // A result from the previous node must not update or promote a new one.
        if self
            .daemon
            .as_ref()
            .and_then(|d| d.config())
            .and_then(local_node_sync_config)
            != Some(&config)
        {
            return Task::none();
        }
        self.bitcoind_sync_probe_in_progress = false;
        match result {
            Err(e) => tracing::warn!("Bitcoind sync check failed: {}", e),
            Ok(LocalNodeSync {
                progress,
                ibd,
                blocks,
                headers,
                subversion,
                pruning,
            }) => {
                self.cache.node_bitcoind_sync_progress = Some(progress);
                self.cache.node_bitcoind_pruning = pruning;
                self.cache.local_switch_history =
                    if history_epoch == self.node_history_epoch && !self.node_history_in_progress {
                        vault_history
                    } else {
                        None
                    };
                self.cache.node_bitcoind_sync_heights = Some((blocks, headers));
                self.cache.node_bitcoind_ibd = Some(ibd);
                // Keep the last good answer if this poll couldn't read it.
                if subversion.is_some() {
                    self.cache.node_bitcoind_subversion = subversion;
                }
                // Auto-switch to the pending node once it's synced, but
                // only if the user *adopted* it (`auto_switch_to_pending`).
                // This fires even for a node that reused an existing
                // chainstate and was therefore never observed in IBD —
                // while never reverting a node merely parked by a
                // switch-to-Connect or a Bitcoind-failure fallback (those
                // clear the flag). Skip while a switch is already in
                // flight, so we don't spawn overlapping switches every
                // tick (which raced to load — and corrupted — the wallet).
                if !ibd
                    && !self.daemon_switch_in_progress
                    && !self.auto_switch_suppressed
                    && (!self.cache.node_history_switch_requested
                        || self.cache.node_history.reconciled)
                    && !self.node_history_in_progress
                    && !self.cache.node_history.job.as_ref().is_some_and(|job| {
                        !matches!(
                            job.phase,
                            crate::node::history::Phase::Complete
                                | crate::node::history::Phase::Cancelled
                        )
                    })
                {
                    let switch = self.daemon.as_ref().and_then(|d| d.config()).and_then(|c| {
                        // Promote unless the node was explicitly parked
                        // (`Some(false)`). An absent flag (`None`) is a
                        // legacy adopted node — the old build promoted it
                        // on sync with no flag — so it must promote too.
                        if c.auto_switch_to_pending == Some(false)
                            && !self.cache.node_history_switch_requested
                        {
                            return None;
                        }
                        let pending = c.pending_bitcoind.clone()?;
                        // Preserve the current Connect config as the new fallback.
                        let old_esplora = match &c.bitcoin_backend {
                            Some(coincubed::config::BitcoinBackend::Esplora(e)) => Some(e.clone()),
                            _ => None,
                        };
                        let mut new_cfg = c.clone();
                        new_cfg.bitcoin_backend =
                            Some(coincubed::config::BitcoinBackend::Bitcoind(pending));
                        new_cfg.pending_bitcoind = None;
                        new_cfg.auto_switch_to_pending = Some(false);
                        new_cfg.fallback_esplora = old_esplora;
                        if new_cfg.bitcoin_config.chain.is_blake2b() {
                            new_cfg.bitcoin_config.poll_interval_secs = Duration::from_secs(
                                coincubed::config::LOCAL_BACKEND_POLL_INTERVAL_SECS,
                            );
                        }
                        Some(new_cfg)
                    });
                    if let Some(new_cfg) = switch {
                        // Not while the Vault is scanning (the switch
                        // would throw the scan away), and never to a
                        // local wallet missing history it cannot rescan.
                        // Deferred, not dropped: every probe re-asks.
                        let hold = local_switch::auto_switch_hold(
                            self.running_vault_scan(),
                            pruning,
                            self.cache.local_switch_history,
                        );
                        if hold != self.local_switch_hold {
                            log_local_switch_hold(hold);
                        }
                        self.local_switch_hold = hold;
                        if hold.is_none() {
                            info!("Switching to local Bitcoind — node synced");
                            return self.spawn_daemon_switch(new_cfg);
                        }
                    }
                }
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincubed::config::{BitcoindConfig, BitcoindRpcAuth};

    fn with_app(test: impl FnOnce(&mut App, BitcoindConfig)) {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = crate::app::shell_tests::bitcoin_app(root.path());
        let pending = BitcoindConfig {
            addr: "127.0.0.1:18333".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::UserPass("fixture".into(), "fixture".into()),
        };
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        cfg.pending_bitcoind = Some(pending.clone());
        cfg.auto_switch_to_pending = Some(true);
        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
        // Model a completed Connect scan, so positive controls may promote.
        app.cache.daemon_cache.blockheight = 900_000;
        app.cache.last_poll_at_startup = Some(100);
        app.cache.daemon_cache.last_poll_timestamp = Some(200);
        assert_eq!(app.running_vault_scan(), None);
        test(&mut app, pending);
    }

    fn observation() -> LocalNodeSync {
        LocalNodeSync {
            progress: 1.0,
            ibd: false,
            blocks: 900_000,
            headers: 900_000,
            subversion: None,
            pruning: Some(local_switch::NodePruning::Unpruned),
        }
    }

    fn message(app: &App, config: BitcoindConfig) -> Message {
        Message::BitcoindSyncProgress {
            app: app.cache.app_generation,
            history_epoch: app.node_history_epoch,
            wallet: app.wallet.as_ref().map(|w| w.id()),
            config,
            result: Ok(observation()),
            vault_history: Some(local_switch::VaultHistory::TrackedLocally),
        }
    }

    #[test]
    fn polling_admits_only_one_probe_and_requires_a_local_config() {
        with_app(|app, _| {
            let first = app.update(Message::PollBitcoindSync);
            assert!(first.units() > 0);
            assert!(app.bitcoind_sync_probe_in_progress);
            assert_eq!(app.update(Message::PollBitcoindSync).units(), 0);
            drop(first); // Lazy task: no real node/RPC is started.
            app.bitcoind_sync_probe_in_progress = false;
            let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
            cfg.pending_bitcoind = None;
            app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
            assert_eq!(app.update(Message::PollBitcoindSync).units(), 0);
            assert!(!app.bitcoind_sync_probe_in_progress);
        });
    }

    #[test]
    fn stale_app_wallet_and_node_cannot_release_or_publish_a_probe() {
        for stale in 0..3 {
            with_app(|app, config| {
                app.bitcoind_sync_probe_in_progress = true;
                let mut reply = message(app, config.clone());
                if let Message::BitcoindSyncProgress {
                    app: origin,
                    wallet,
                    config,
                    ..
                } = &mut reply
                {
                    match stale {
                        0 => *origin = cache::AppGeneration::next(),
                        1 => *wallet = None,
                        _ => config.addr = "127.0.0.1:18334".parse().unwrap(),
                    }
                }
                assert_eq!(app.update(reply).units(), 0);
                assert!(app.bitcoind_sync_probe_in_progress);
                assert_eq!(app.cache.node_bitcoind_sync_heights, None);
                assert_eq!(app.cache.local_switch_history, None);
                assert!(!app.daemon_switch_in_progress);
                let reply = message(app, config);
                let task = app.update(reply);
                assert!(!app.bitcoind_sync_probe_in_progress);
                assert_eq!(
                    app.cache.node_bitcoind_sync_heights,
                    Some((900_000, 900_000))
                );
                assert!(app.daemon_switch_in_progress);
                assert!(task.units() > 0);
                drop(task);
            });
        }
    }

    #[test]
    fn history_epoch_or_inflight_history_revokes_permission_but_keeps_telemetry() {
        for inflight in [false, true] {
            with_app(|app, config| {
                app.auto_switch_suppressed = true;
                app.cache.node_bitcoind_subversion = Some("last-good".into());
                app.cache.local_switch_history = Some(local_switch::VaultHistory::From(10));
                let reply = message(app, config.clone());
                if inflight {
                    app.node_history_in_progress = true;
                } else {
                    app.node_history_epoch += 1;
                }
                drop(app.update(reply));
                assert_eq!(app.cache.local_switch_history, None);
                assert_eq!(
                    app.cache.node_bitcoind_sync_heights,
                    Some((900_000, 900_000))
                );
                assert_eq!(
                    app.cache.node_bitcoind_subversion.as_deref(),
                    Some("last-good")
                );
                app.node_history_in_progress = false;
                let reply = message(app, config);
                drop(app.update(reply));
                assert_eq!(
                    app.cache.local_switch_history,
                    Some(local_switch::VaultHistory::TrackedLocally)
                );
            });
        }
    }

    #[test]
    fn failed_probe_releases_guard_without_erasing_last_good_data() {
        with_app(|app, config| {
            app.auto_switch_suppressed = true;
            let reply = message(app, config.clone());
            drop(app.update(reply));
            app.bitcoind_sync_probe_in_progress = true;
            let mut reply = message(app, config);
            if let Message::BitcoindSyncProgress { result, .. } = &mut reply {
                *result = Err("unreachable".into());
            }
            drop(app.update(reply));
            assert!(!app.bitcoind_sync_probe_in_progress);
            assert_eq!(
                app.cache.node_bitcoind_sync_heights,
                Some((900_000, 900_000))
            );
            assert_eq!(
                app.cache.local_switch_history,
                Some(local_switch::VaultHistory::TrackedLocally)
            );
        });
    }

    #[test]
    fn parked_suppressed_or_switching_nodes_do_not_autopromote() {
        for refusal in 0..3 {
            with_app(|app, config| {
                match refusal {
                    0 => {
                        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
                        cfg.auto_switch_to_pending = Some(false);
                        app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
                    }
                    1 => app.auto_switch_suppressed = true,
                    _ => app.daemon_switch_in_progress = true,
                }
                let reply = message(app, config.clone());
                assert_eq!(app.update(reply).units(), 0);
                assert_eq!(app.cache.node_bitcoind_ibd, Some(false));
                app.auto_switch_suppressed = false;
                app.daemon_switch_in_progress = false;
                let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
                cfg.auto_switch_to_pending = Some(true);
                app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None)));
                let reply = message(app, config);
                let task = app.update(reply);
                assert!(task.units() > 0);
                assert!(app.daemon_switch_in_progress);
                drop(task);
            });
        }
    }
}
