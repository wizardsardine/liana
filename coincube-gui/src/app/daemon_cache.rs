//! Publishes daemon snapshots and handles unavailable local-node fallbacks.
use super::*;

impl App {
    pub(super) fn handle_daemon_cache(
        &mut self,
        result: Result<DaemonCache, Error>,
    ) -> Task<Message> {
        if self.cache.has_imported_bitcoin {
            return Task::none();
        }
        match result {
            Ok(cache) => self.publish_daemon_cache(cache),
            Err(error) => self.daemon_cache_failed(error),
        }
    }

    fn publish_daemon_cache(&mut self, mut daemon_cache: DaemonCache) -> Task<Message> {
        // Apply optimistic-broadcast overrides before the
        // cache is published: reconcile drops entries the
        // daemon now reflects on its own, then any still-
        // pending broadcasts get synthetic `spend_info` so
        // `coins_summary` (Vault balance) and every other
        // `cache.coins()` consumer treats the inputs as
        // already spent.
        if let Some(wallet) = &self.wallet {
            wallet.reconcile_with_coins(&daemon_cache.coins);
            wallet.apply_coin_overrides(&mut daemon_cache.coins);
        }
        let retire_obligation = if full_scan_settles_rescan_obligation(
            &self.daemon_backend(),
            self.rescan_obligation_retired_by_scan || self.rescan_obligation_retire_in_flight,
            &daemon_cache.history_sync,
        ) {
            let task = self.retire_rescan_obligation_task();
            self.rescan_obligation_retire_in_flight = task.is_some();
            task
        } else {
            None
        };
        let vault_changed =
            overview_activity_vault_changed(&self.cache.daemon_cache, &daemon_cache);
        self.cache.daemon_cache = daemon_cache;
        // The Vault page's rows follow its own reload
        // (`Message::Payments`).
        let recent = if vault_changed
            && matches!(self.panels.current, Menu::Cube(menu::CubeSubMenu::Overview))
        {
            self.panels.activity.refresh_wallet(
                view::activity::WalletFilter::Vault,
                self.daemon.clone(),
                self.wallet.clone(),
            )
        } else {
            Task::none()
        };
        // Fire-and-forget recovery heartbeat after the sync's
        // fresh tip lands (Estate Notifications — PR 2). Batched
        // alongside the normal cache cascade so it never delays
        // or blocks it.
        let heartbeat = self.recovery_heartbeat_task();
        // Same posture for the BTCB2 entangled-deposit lookups:
        // `Task::none()` on every Bitcoin-family Cube.
        let entangled = self.entangled_lookup_task();
        let chain_proofs = self.chain_proof_task();
        let splits = self.split_refresh_task();
        let unswept = self.unswept_notice_task();
        Task::batch([
            heartbeat,
            entangled,
            chain_proofs,
            splits,
            unswept,
            retire_obligation.unwrap_or_else(Task::none),
            recent,
            Task::done(Message::CacheUpdated),
        ])
    }

    fn daemon_cache_failed(&mut self, error: Error) -> Task<Message> {
        tracing::error!("Failed to update daemon cache: {}", error);
        // A transient RPC response cannot demote a healthy local node.
        if is_daemon_unreachable(&error) {
            let fallback = self.daemon_fallback_config();
            if let Some(new_cfg) = fallback {
                if !self.daemon_switch_in_progress {
                    info!("Switching to COINCUBE | Connect fallback after Bitcoind failure");
                    return self.spawn_daemon_switch(new_cfg);
                }
            }
        }
        Task::none()
    }

    fn daemon_fallback_config(&self) -> Option<DaemonConfig> {
        self.daemon
            .as_ref()
            .filter(|d| {
                matches!(
                    d.backend(),
                    DaemonBackend::EmbeddedCoincubed(Some(NodeType::Bitcoind))
                )
            })
            .and_then(|d| d.config())
            .and_then(|c| {
                c.fallback_esplora.as_ref().map(|fb| {
                    let mut new_cfg = c.clone();
                    // Demote the current Bitcoind to
                    // `pending_bitcoind` so the syncing card
                    // reappears and the user can retry once
                    // the node is healthy. Without this the
                    // fallback strands the user on Connect
                    // with an empty pending slot, which
                    // surfaces the "Set up local node" prompt
                    // and forces a full re-install.
                    let preserved_bitcoind = match &c.bitcoin_backend {
                        Some(coincubed::config::BitcoinBackend::Bitcoind(bc)) => Some(bc.clone()),
                        _ => None,
                    };
                    new_cfg.bitcoin_backend =
                        Some(coincubed::config::BitcoinBackend::Esplora(fb.clone()));
                    new_cfg.pending_bitcoind = preserved_bitcoind;
                    // Fell back to Connect after a Bitcoind
                    // failure — park the node but don't
                    // auto-revert to it on the next probe.
                    new_cfg.auto_switch_to_pending = Some(false);
                    new_cfg.fallback_esplora = None;
                    new_cfg
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::model::Coin;
    use coincubed::config::{BitcoinBackend, BitcoindConfig};
    use std::str::FromStr;

    fn with_app(test: impl FnOnce(&mut App, Arc<Wallet>)) {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, wallet) = crate::app::shell_tests::bitcoin_app(root.path());
        test(&mut app, wallet);
    }

    fn coin() -> Coin {
        Coin {
            amount: bitcoin::Amount::from_sat(10_000),
            outpoint: bitcoin::OutPoint::null(),
            address: bitcoin::Address::from_str(
                "tb1qgpuzqmt7jgdk2t426e9uwfv68uyukh48nfq4qyl74rh0v8cn44rs4wnj68",
            )
            .unwrap()
            .assume_checked(),
            block_height: Some(10),
            derivation_index: bitcoin::bip32::ChildNumber::from_normal_idx(0).unwrap(),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    #[test]
    fn optimistic_spend_is_applied_before_activity_comparison_and_publication() {
        with_app(|app, wallet| {
            let coin = coin();
            let tx = bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![bitcoin::TxIn {
                    previous_output: coin.outpoint,
                    ..Default::default()
                }],
                output: vec![],
            };
            let txid = tx.compute_txid();
            wallet.record_broadcast(tx, vec![coin.clone()], vec![], bitcoin::Network::Bitcoin);
            app.cache.daemon_cache.coins = vec![coin.clone()];
            app.panels.current = Menu::Cube(menu::CubeSubMenu::Overview);
            let generation = app.panels.activity.generation_for_tests();
            drop(app.update(Message::ActivityVaultLoaded(
                generation,
                Ok((vec![], false)),
            )));
            assert!(!app.panels.activity.vault_loading);
            let snapshot = DaemonCache {
                coins: vec![coin.clone()],
                ..Default::default()
            };
            drop(app.update(Message::UpdateDaemonCache(Ok(snapshot))));
            assert_eq!(app.cache.coins()[0].spend_info.unwrap().txid, txid);
            assert!(
                app.panels.activity.vault_loading,
                "optimistic spend must be visible to the history comparison"
            );
            // When the daemon removes the spent input, reconciliation must also
            // retire the override; a later sighting cannot resurrect it.
            drop(app.update(Message::UpdateDaemonCache(Ok(DaemonCache::default()))));
            let mut probe = vec![coin];
            wallet.apply_coin_overrides(&mut probe);
            assert!(probe[0].spend_info.is_none());
        });
    }

    fn local_with_fallback(app: &App) -> DaemonConfig {
        let mut cfg = app.daemon.as_ref().unwrap().config().unwrap().clone();
        let Some(BitcoinBackend::Esplora(fallback)) = cfg.bitcoin_backend.clone() else {
            panic!("fixture must start on Esplora");
        };
        cfg.fallback_esplora = Some(fallback);
        cfg.bitcoin_backend = Some(BitcoinBackend::Bitcoind(BitcoindConfig {
            addr: "127.0.0.1:18332".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::UserPass("fixture".into(), "fixture".into()),
        }));
        cfg.auto_switch_to_pending = Some(true);
        cfg
    }

    #[test]
    fn fallback_preserves_local_node_and_refuses_rpc_errors_or_duplicate_switches() {
        with_app(|app, _| {
            let cfg = local_with_fallback(app);
            app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
                cfg.clone(),
                None,
            )));
            let fallback = app
                .daemon_fallback_config()
                .expect("eligible local-node fallback");
            let Some(BitcoinBackend::Bitcoind(local)) = cfg.bitcoin_backend.as_ref() else {
                unreachable!()
            };
            assert_eq!(fallback.pending_bitcoind.as_ref(), Some(local));
            assert_eq!(fallback.auto_switch_to_pending, Some(false));
            assert!(fallback.fallback_esplora.is_none());
            assert!(matches!(
                fallback.bitcoin_backend,
                Some(BitcoinBackend::Esplora(_))
            ));
            assert!(app
                .daemon
                .as_ref()
                .unwrap()
                .config()
                .unwrap()
                .fallback_esplora
                .is_some());

            let rpc = Error::Daemon(DaemonError::Rpc(-1, "application error".into()));
            assert_eq!(app.update(Message::UpdateDaemonCache(Err(rpc))).units(), 0);
            assert!(!app.daemon_switch_in_progress);
            app.daemon_switch_in_progress = true;
            assert_eq!(
                app.update(Message::UpdateDaemonCache(Err(Error::Daemon(
                    DaemonError::NoAnswer
                ))))
                .units(),
                0
            );
            assert!(app.daemon_switch_in_progress);
            app.daemon_switch_in_progress = false;
            // Positive control uses the same eligible configuration. Drop the
            // lazy task: no daemon is started and no RPC/network call runs.
            let task = app.update(Message::UpdateDaemonCache(Err(Error::Daemon(
                DaemonError::NoAnswer,
            ))));
            assert!(task.units() > 0);
            assert!(app.daemon_switch_in_progress);
            drop(task);
        });
    }

    #[test]
    fn imported_accounts_ignore_vault_snapshots_and_nonlocal_backends_do_not_fallback() {
        with_app(|app, _| {
            app.cache.daemon_cache.blockheight = 7;
            app.cache.has_imported_bitcoin = true;
            let task = app.update(Message::UpdateDaemonCache(Ok(DaemonCache {
                blockheight: 99,
                ..Default::default()
            })));
            assert_eq!(task.units(), 0);
            assert_eq!(app.cache.daemon_cache.blockheight, 7);
            app.cache.has_imported_bitcoin = false;
            assert!(app.daemon_fallback_config().is_none());
            assert_eq!(
                app.update(Message::UpdateDaemonCache(Err(Error::Daemon(
                    DaemonError::NoAnswer
                ))))
                .units(),
                0
            );
            assert!(!app.daemon_switch_in_progress);
        });
    }
}
