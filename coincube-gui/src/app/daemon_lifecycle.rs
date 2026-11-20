//! Handles daemon replacement results without mixing them into the dispatcher.
use super::*;

impl App {
    /// Invalidate history probes and switch consent tied to the previous Vault or backend.
    pub(super) fn invalidate_node_history(&mut self) {
        self.bitcoind_sync_probe_in_progress = false;
        self.node_history_request_epoch = self.node_history_request_epoch.wrapping_add(1);
        self.node_history_epoch = self.node_history_epoch.wrapping_add(1);
        self.node_history_in_progress = false;
        self.node_history_pending = None;
        self.cache.node_history = node_history::Status::default();
        self.cache.node_history_switch_requested = false;
        self.cache.local_switch_history = None;
        self.local_switch_hold = None;
    }

    pub(super) fn handle_daemon_restarted(&mut self, outcome: DaemonRestart) -> Task<Message> {
        self.daemon_switch_in_progress = false;
        self.cache.daemon_switch_in_progress = false;
        // Non-blocking toast emitted on top of the normal result (e.g. a
        // switch that succeeded but couldn't be persisted to disk).
        let mut extra = Task::none();
        let result = match outcome {
            DaemonRestart::Started(daemon) => {
                self.daemon = Some(daemon);
                // A fresh successful switch (adopt / manual) re-arms
                // auto-promotion that a prior failure had suppressed.
                self.auto_switch_suppressed = false;
                Ok(())
            }
            DaemonRestart::StartedNotPersisted(daemon) => {
                // The switch succeeded (wallet is on the new backend); the
                // only problem is the config wasn't saved. Report success
                // but warn that it may not survive a restart.
                self.daemon = Some(daemon);
                self.auto_switch_suppressed = false;
                extra = Task::done(Message::View(view::Message::ShowToast(
                    log::Level::Warn,
                    "Switched Bitcoin backend, but couldn't save the change to disk — \
                     it may revert if you restart Coincube."
                        .to_string(),
                )));
                Ok(())
            }
            DaemonRestart::Failed { error, recovered } => {
                if let Some(daemon) = recovered {
                    self.daemon = Some(daemon);
                    // The recovered daemon still carries the armed
                    // `auto_switch_to_pending` + `pending_bitcoind`, so
                    // suppress auto-promotion to stop the same failing
                    // switch re-firing every poll. A later user-initiated
                    // switch re-arms it (see the Started arms).
                    self.auto_switch_suppressed = true;
                } else {
                    // The old daemon was already stopped during the switch
                    // and recovery couldn't bring one back. Drop it rather
                    // than keep referencing a dead daemon that ticks and
                    // config loads would keep poking.
                    self.daemon = None;
                }
                error!("Daemon backend switch failed: {}", error);
                Err(error)
            }
            DaemonRestart::Panicked(error) => {
                // Unknown post-panic state. self.daemon still holds the
                // pre-switch daemon (cloned, not taken), so leave it in
                // place — a possibly-stopped backend the user can re-switch
                // beats no backend on an open vault. Suppress auto-promotion
                // so a still-armed config can't re-trigger the same panic
                // every poll; a manual switch re-arms it.
                self.auto_switch_suppressed = true;
                error!("Daemon backend switch panicked; keeping previous daemon: {error}");
                Err(error)
            }
        };
        // A successful switch invalidates the previous node probe and
        // clears its cached permission before polling the replacement.
        if result.is_ok() {
            self.invalidate_node_history();
            self.cache.node_bitcoind_sync_progress = None;
            self.cache.node_bitcoind_sync_heights = None;
            self.cache.node_bitcoind_ibd = None;
            self.cache.node_bitcoind_subversion = None;
            self.cache.node_bitcoind_pruning = None;
            self.cache.node_bitcoind_last_log = None;
        }
        // An imported wallet's panel holds the runtime for its scan and
        // signing work; hand it the one now running. When the switch left no
        // daemon at all, the panel must let go of the stopped one too, or a
        // later refresh would hand it back to the App.
        let imported_task = match (self.daemon.clone(), self.panels.imported_bitcoin.as_mut()) {
            (Some(daemon), Some(panel)) => panel.replace_runtime(daemon),
            (None, Some(panel)) => {
                panel.detach_runtime();
                Task::none()
            }
            _ => Task::none(),
        };
        let cfg_task = self.update_dispatch(Message::DaemonConfigLoaded(result));
        Task::batch([
            cfg_task,
            imported_task,
            extra,
            Task::done(Message::CacheUpdated),
            Task::done(Message::PollBitcoindSync),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_app(test: impl FnOnce(&mut App)) {
        let _guard = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = crate::app::shell_tests::bitcoin_app(root.path());
        app.daemon_switch_in_progress = true;
        app.cache.daemon_switch_in_progress = true;
        app.bitcoind_sync_probe_in_progress = true;
        app.node_history_request_epoch = u64::MAX;
        app.node_history_in_progress = true;
        app.node_history_pending = Some(node_history::Action::Pause);
        app.cache.node_history.busy = true;
        app.cache.node_history.error = Some("old probe".into());
        app.cache.node_history_switch_requested = true;
        app.cache.local_node_transfer_visible = true;
        app.cache.node_bitcoind_sync_progress = Some(0.75);
        app.cache.node_bitcoind_sync_heights = Some((30, 40));
        app.cache.node_bitcoind_ibd = Some(true);
        app.cache.node_bitcoind_subversion = Some("old node".into());
        app.cache.node_bitcoind_pruning =
            Some(local_switch::NodePruning::Pruned { prune_height: 5 });
        app.cache.node_bitcoind_last_log = Some("old log".into());
        test(&mut app);
    }

    #[test]
    fn both_success_outcomes_install_backend_and_invalidate_previous_probes() {
        for not_persisted in [false, true] {
            with_app(|app| {
                app.auto_switch_suppressed = true;
                let replacement = crate::app::shell_tests::fixture_daemon(app);
                app.daemon = None;
                let outcome = if not_persisted {
                    DaemonRestart::StartedNotPersisted(replacement.clone())
                } else {
                    DaemonRestart::Started(replacement.clone())
                };
                drop(app.update(Message::DaemonRestarted(outcome)));
                assert!(Arc::ptr_eq(app.daemon.as_ref().unwrap(), &replacement));
                assert!(!app.daemon_switch_in_progress);
                assert!(!app.cache.daemon_switch_in_progress);
                assert!(!app.auto_switch_suppressed);
                assert!(!app.bitcoind_sync_probe_in_progress);
                assert_eq!(app.node_history_request_epoch, 0);
                assert!(!app.node_history_in_progress);
                assert!(app.node_history_pending.is_none());
                assert!(!app.cache.node_history.busy);
                assert!(app.cache.node_history.error.is_none());
                assert!(!app.cache.node_history_switch_requested);
                assert!(app.cache.local_node_transfer_visible);
                assert!(app.cache.node_bitcoind_sync_progress.is_none());
                assert!(app.cache.node_bitcoind_sync_heights.is_none());
                assert!(app.cache.node_bitcoind_ibd.is_none());
                assert!(app.cache.node_bitcoind_subversion.is_none());
                assert!(app.cache.node_bitcoind_pruning.is_none());
                assert!(app.cache.node_bitcoind_last_log.is_none());
            });
        }
    }

    #[test]
    fn daemon_restart_rejects_pre_restart_history_from_the_same_node() {
        use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth};

        with_app(|app| {
            let local = BitcoindConfig {
                addr: "127.0.0.1:18333".parse().unwrap(),
                rpc_auth: BitcoindRpcAuth::UserPass("fixture".into(), "fixture".into()),
            };
            let mut config = app.daemon.as_ref().unwrap().config().unwrap().clone();
            config.pending_bitcoind = Some(local.clone());
            app.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(
                config.clone(),
                None,
            )));
            let previous_epoch = app.node_history_epoch;

            // Promotion changes the daemon while retaining the node being probed.
            config.pending_bitcoind = None;
            config.bitcoin_backend = Some(BitcoinBackend::Bitcoind(local.clone()));
            drop(
                app.update(Message::DaemonRestarted(DaemonRestart::Started(Arc::new(
                    EmbeddedDaemon::unstarted_for_test(config, None),
                )))),
            );

            let reply = |app: &App, history_epoch| Message::BitcoindSyncProgress {
                app: app.cache.app_generation,
                history_epoch,
                wallet: app.wallet.as_ref().map(|wallet| wallet.id()),
                config: local.clone(),
                result: Ok(LocalNodeSync {
                    progress: 1.0,
                    ibd: false,
                    blocks: 100,
                    headers: 100,
                    subversion: None,
                    pruning: Some(local_switch::NodePruning::Pruned { prune_height: 1 }),
                }),
                vault_history: Some(local_switch::VaultHistory::TrackedLocally),
            };
            drop(app.update(reply(app, previous_epoch)));
            assert_eq!(app.cache.local_switch_history, None);
            assert_eq!(app.cache.node_bitcoind_sync_heights, Some((100, 100)));

            drop(app.update(reply(app, app.node_history_epoch)));
            assert_eq!(
                app.cache.local_switch_history,
                Some(local_switch::VaultHistory::TrackedLocally)
            );
        });
    }

    #[test]
    fn unsuccessful_outcomes_preserve_probes_and_distinguish_backend_ownership() {
        for shape in ["recovered", "unrecovered", "panic"] {
            for suppressed in [false, true] {
                with_app(|app| {
                    app.auto_switch_suppressed = suppressed;
                    let previous = app.daemon.as_ref().unwrap().clone();
                    let error = Error::Config("fixture restart failure".into());
                    let outcome = match shape {
                        "recovered" => DaemonRestart::Failed {
                            error,
                            recovered: Some(previous.clone()),
                        },
                        "unrecovered" => DaemonRestart::Failed {
                            error,
                            recovered: None,
                        },
                        _ => DaemonRestart::Panicked(error),
                    };
                    drop(app.update(Message::DaemonRestarted(outcome)));
                    assert!(!app.daemon_switch_in_progress);
                    assert!(!app.cache.daemon_switch_in_progress);
                    if shape == "unrecovered" {
                        assert!(app.daemon.is_none());
                        assert_eq!(app.auto_switch_suppressed, suppressed);
                    } else {
                        assert!(Arc::ptr_eq(app.daemon.as_ref().unwrap(), &previous));
                        assert!(app.auto_switch_suppressed);
                    }
                    assert!(app.bitcoind_sync_probe_in_progress);
                    assert_eq!(app.node_history_request_epoch, u64::MAX);
                    assert!(app.node_history_in_progress);
                    assert!(matches!(
                        app.node_history_pending,
                        Some(node_history::Action::Pause)
                    ));
                    assert!(app.cache.node_history.busy);
                    assert_eq!(app.cache.node_history.error.as_deref(), Some("old probe"));
                    assert!(app.cache.node_history_switch_requested);
                    assert_eq!(app.cache.node_bitcoind_sync_progress, Some(0.75));
                    assert_eq!(app.cache.node_bitcoind_sync_heights, Some((30, 40)));
                    assert_eq!(app.cache.node_bitcoind_ibd, Some(true));
                    assert_eq!(
                        app.cache.node_bitcoind_subversion.as_deref(),
                        Some("old node")
                    );
                    assert_eq!(
                        app.cache.node_bitcoind_pruning,
                        Some(local_switch::NodePruning::Pruned { prune_height: 5 })
                    );
                    assert_eq!(app.cache.node_bitcoind_last_log.as_deref(), Some("old log"));
                });
            }
        }
    }
}
