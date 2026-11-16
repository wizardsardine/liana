//! App-owned recovery controller. Panel navigation does not stop a saved job.
use super::wallet::Wallet;
use crate::{
    chain::ChainId,
    daemon::Daemon,
    dir::CoincubeDirectory,
    node::{
        bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_datadir, Bitcoind,
            InternalBitcoindConfig,
        },
        history::{self, NodeRpc, Phase, Recovery},
        retention,
    },
};
use coincube_core::miniscript::bitcoin::Network;
use coincubed::{
    commands::CoinStatus,
    config::{BitcoinBackend, BitcoindConfig},
};
use std::{collections::BTreeSet, convert::TryFrom, path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub enum Action {
    Poll,
    Import,
    Recover(String),
    Retain(u32),
    Resume,
    Pause,
    Cancel,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub job: Option<Recovery>,
    pub policy: Option<retention::Policy>,
    pub error: Option<String>,
    pub busy: bool,
    pub cancel_queued: bool,
    pub reconciled: bool,
}

#[derive(Debug)]
pub struct Outcome {
    pub status: Status,
    pub node: Option<Bitcoind>,
}

pub struct Context {
    pub datadir: CoincubeDirectory,
    pub network: Network,
    pub chain: ChainId,
    pub config: BitcoindConfig,
    pub managed_owner: Option<PathBuf>,
    pub daemon: Arc<dyn Daemon + Send + Sync>,
    pub wallet: Arc<Wallet>,
}

const OTHER_CUBE_OWNS_NODE: &str =
    "Another Cube owns the managed node. Close it before changing node settings here.";

fn record_temporary_finish_error(status: &mut Status, error: String) {
    if error == OTHER_CUBE_OWNS_NODE {
        tracing::debug!("Deferring temporary retention finish while another Cube owns the node");
    } else {
        status.error = Some(error);
    }
}

pub fn parse_start(value: &str) -> Result<Start, String> {
    if let Ok(height) = value.trim().parse::<u32>() {
        return Ok(Start::Height(height));
    }
    let date = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| "Enter a block height or a date in YYYY-MM-DD format")?;
    let timestamp = date
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid recovery date")?
        .and_utc()
        .timestamp();
    Ok(Start::Date(
        u64::try_from(timestamp).map_err(|_| "Recovery date must be after 1970")?,
    ))
}
pub enum Start {
    Height(u32),
    Date(u64),
}

impl Context {
    async fn exclusive_lifecycle(
        &self,
    ) -> Result<crate::node::bitcoind::ManagedNodeLifecycle, String> {
        let root = self.datadir.clone();
        let owner = self.managed_owner.clone();
        tokio::task::spawn_blocking(move || {
            let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
                &root,
                crate::node::bitcoind::NodeChainFamily::Bitcoin,
            )
            .map_err(|e| e.to_string())?;
            lifecycle.ensure_exclusive_settings_owner(owner.as_deref())?;
            Ok::<_, String>(lifecycle)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub async fn run(self, action: Action) -> Result<Outcome, String> {
        let vault = self
            .wallet
            .inheritance_descriptor()
            .ok_or("Node recovery requires a Cube Vault with inheritance policy")?;
        let config = self
            .daemon
            .config()
            .ok_or("Recovery requires an embedded wallet configuration")?;
        let wallet_path = coincubed::bitcoind_watchonly_wallet_path(
            &config
                .data_directory()
                .ok_or("Missing wallet data directory")?,
        );
        let binding = retention::managed_binding(
            &self.datadir,
            self.network,
            self.chain,
            &self.config,
            &wallet_path,
            &vault.to_string(),
        )?;
        let directory = retention::directory(&self.datadir, self.network);
        let path = history::recovery_path(&directory, &wallet_path);
        let mut status = Status {
            job: history::read_json(&path)?,
            policy: retention::load(&self.datadir, self.network)?,
            ..Status::default()
        };
        if let Some(job) = &status.job {
            job.validate(&binding)?;
        }
        if matches!(action, Action::Poll) && status.job.is_none() && status.policy.is_none() {
            return Ok(Outcome { status, node: None });
        }
        let mut rpc = NodeRpc::new(&self.config, &wallet_path, self.chain).await?;
        let mut node = None;

        if matches!(action, Action::Pause | Action::Cancel) {
            let _lease = history::HistoryLease::acquire(&directory)?;
            let mut job: Recovery = history::read_json(&path)?.ok_or("No recovery is running")?;
            job.validate(&binding)?;
            if matches!(action, Action::Pause)
                && matches!(job.phase, Phase::Complete | Phase::Cancelled)
            {
                return Ok(Outcome { status, node });
            }
            job.phase = if matches!(action, Action::Cancel) {
                Phase::Cancelled
            } else {
                Phase::Paused
            };
            job.last_error = None;
            job.transient_failures = 0;
            history::write_json(&path, &job)?;
            status.job = Some(job);
            if !matches!(action, Action::Cancel) {
                return Ok(Outcome { status, node });
            }
        }

        let starting = matches!(
            action,
            Action::Import | Action::Recover(_) | Action::Retain(_)
        );
        if starting || matches!(action, Action::Resume) {
            if matches!(action, Action::Resume)
                && status
                    .job
                    .as_ref()
                    .is_none_or(|job| matches!(job.phase, Phase::Complete | Phase::Cancelled))
            {
                return Err("No interrupted recovery is available to resume".into());
            }
            let info = rpc.check_chain(self.network).await?;
            let tip = u32::try_from(info["blocks"].as_u64().ok_or("Missing local chain tip")?)
                .map_err(|_| "Local chain height is out of range")?;
            let coins = self
                .daemon
                .list_coins(
                    &[
                        CoinStatus::Unconfirmed,
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent,
                    ],
                    &[],
                )
                .await
                .map_err(|e| e.to_string())?
                .coins;
            let mut settings_lifecycle = None;
            if starting {
                if status
                    .job
                    .as_ref()
                    .is_some_and(|job| !matches!(job.phase, Phase::Complete | Phase::Cancelled))
                {
                    return Err("Pause and resume the current recovery, or cancel it before starting another".into());
                }
                let start = match &action {
                    Action::Recover(value) => match parse_start(value)? {
                        Start::Height(h) => h,
                        Start::Date(t) => {
                            if t > now() {
                                return Err("Recovery date cannot be in the future".into());
                            }
                            retention::height_from_date(&rpc, t, tip).await?
                        }
                    },
                    Action::Retain(days) => {
                        if !(1..=3650).contains(days) {
                            return Err("Choose between 1 and 3650 retention days".into());
                        }
                        retention::height_from_date(
                            &rpc,
                            now().saturating_sub(u64::from(*days) * 86400),
                            tip,
                        )
                        .await?
                    }
                    _ => coins
                        .iter()
                        .filter_map(|coin| coin.block_height.and_then(|h| u32::try_from(h).ok()))
                        .min()
                        .unwrap_or(tip.saturating_sub(history::BATCH_BLOCKS - 1)),
                };
                let mut job =
                    Recovery::new(binding.clone(), start, tip, rpc.block_hash(tip).await?)?;
                // A crash during setup leaves an explicit resumable obligation.
                // Other Cubes preserve this range while setup/import runs.
                job.phase = Phase::Paused;
                job.import_requested = matches!(action, Action::Import);
                job.last_error = Some("Recovery setup was interrupted; resume to continue".into());
                settings_lifecycle = Some(self.exclusive_lifecycle().await?);
                let _lease = history::HistoryLease::acquire(&directory)?;
                history::write_json(&path, &job)?;
                status.job = Some(job);
            }
            if settings_lifecycle.is_none() {
                settings_lifecycle = Some(self.exclusive_lifecycle().await?);
            }
            status.policy = Some(match action {
                Action::Retain(days) => retention::configure(
                    &self.datadir,
                    self.network,
                    self.chain,
                    &self.config,
                    days,
                )?,
                _ => retention::configure_recovery(
                    &self.datadir,
                    self.network,
                    self.chain,
                    &self.config,
                    u32::try_from(
                        info["pruneheight"]
                            .as_u64()
                            .ok_or("Missing local pruning height")?,
                    )
                    .map_err(|_| "Invalid pruning height")?,
                )?,
            });
            if info["automatic_pruning"].as_bool() != Some(false) {
                let conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(
                    &internal_bitcoind_datadir(&self.datadir),
                ))
                .map_err(|e| e.to_string())?;
                let (cfg, started) =
                    super::state::vault::settings::bitcoind::ensure_tor_and_start_managed_with_lifecycle(
                        self.datadir.clone(),
                        self.network,
                        conf.flavor,
                        None,
                        true,
                        None,
                        self.managed_owner.clone(),
                        settings_lifecycle.take(),
                    )
                    .await?;
                if cfg != self.config {
                    return Err("Managed node endpoint changed during recovery setup; retry".into());
                }
                node = Some(started);
                rpc = NodeRpc::new(&self.config, &wallet_path, self.chain).await?;
            }
            drop(settings_lifecycle);
            let local = rpc.check_chain(self.network).await?;
            if local["pruned"].as_bool() != Some(true)
                || local["automatic_pruning"].as_bool() != Some(false)
            {
                return Err("The managed node did not enable recovery pruning controls".into());
            }
            let _lease = history::HistoryLease::acquire(&directory)?;
            let indices = self.daemon.get_info().await.map_err(|e| e.to_string())?;
            let range = indices
                .receive_index
                .max(indices.change_index)
                .max(
                    coins
                        .iter()
                        .map(|coin| u32::from(coin.derivation_index))
                        .max()
                        .unwrap_or(0),
                )
                .saturating_add(200)
                .max(1000);
            if range >= 1 << 31 {
                return Err("Wallet derivation range is too large".into());
            }
            let mut job: Recovery =
                history::read_json(&path)?.ok_or("Recovery progress is missing")?;
            job.validate(&binding)?;
            if !rpc.ensure_loaded(&wallet_path).await? {
                job.phase = Phase::FetchingForLoad;
                job.last_error = None;
                history::write_json(&path, &job)?;
                status.job = Some(job);
                return Ok(Outcome { status, node });
            }
            rpc.prepare_wallet(&wallet_path, vault, range).await?;
            job.wallet_ready = true;
            if job.import_requested {
                let source = history::outspend_source(config)
                    .ok_or("No Connect/Esplora source is configured for transaction proofs")?;
                let txids: BTreeSet<_> = coins
                    .iter()
                    .flat_map(|coin| {
                        std::iter::once(coin.outpoint.txid)
                            .chain(coin.spend_info.map(|spend| spend.txid))
                    })
                    .collect();
                let mut transactions = vec![];
                for batch in txids.into_iter().collect::<Vec<_>>().chunks(64) {
                    transactions.extend(
                        self.daemon
                            .list_txs(batch)
                            .await
                            .map_err(|e| e.to_string())?
                            .transactions,
                    );
                }
                // Blocks whose transaction had no proof or was refused (a spend
                // with no change back to this Vault) are scanned one by one.
                let unproved = history::import_transactions(&rpc, source, &transactions).await?;
                let mut plan = history::plan_coin_scans(&rpc, &coins, Some(source)).await?;
                plan.blocks.extend(unproved);
                // Even complete proof imports get a final tail/mempool scan,
                // started lower only when some coin needs every block.
                let mut tail = tip.saturating_sub(history::BATCH_BLOCKS - 1);
                let mut fallback = None;
                if let Some((floor, reason)) = plan.floor {
                    if floor < tail {
                        tail = floor;
                        fallback = Some(reason);
                    }
                }
                job.plan(plan.blocks, tail, fallback);
                job.import_requested = false;
            }
            job.phase = if job.next > job.target {
                Phase::Reconciling
            } else {
                job.db_replayed = false;
                Phase::Downloading
            };
            job.fetch_attempts = 0;
            job.last_error = None;
            job.transient_failures = 0;
            history::write_json(&path, &job)?;
            status.job = Some(job);
        } else if let Some(job) = &status.job {
            if !matches!(job.phase, Phase::Cancelled | Phase::Paused) {
                if job.phase != Phase::FetchingForLoad && !rpc.ensure_loaded(&wallet_path).await? {
                    let _lease = history::HistoryLease::acquire(&directory)?;
                    let mut saved: Recovery =
                        history::read_json(&path)?.ok_or("Missing recovery progress")?;
                    saved.phase = Phase::FetchingForLoad;
                    saved.wallet_ready = false;
                    saved.rewind(Some(
                        "The local wallet had to be reloaded, so COINCUBE Desktop scans every block again."
                            .into(),
                    ));
                    history::write_json(&path, &saved)?;
                    status.job = Some(saved);
                    return Ok(Outcome { status, node });
                }
                if job.phase == Phase::FetchingForLoad && job.download_next > job.target {
                    // Re-enter preparation only after every catch-up body was
                    // retained. If this range was too recent Core still refuses.
                    if !rpc.ensure_loaded(&wallet_path).await? {
                        let _lease = history::HistoryLease::acquire(&directory)?;
                        let mut saved: Recovery =
                            history::read_json(&path)?.ok_or("Missing recovery progress")?;
                        saved.phase = Phase::Paused;
                        saved.last_error=Some("This wallet needs blocks earlier than the selected start. Cancel this recovery and choose an earlier height or date.".into());
                        history::write_json(&path, &saved)?;
                        status.job = Some(saved);
                    } else {
                        return Box::pin(self.run(Action::Resume)).await;
                    }
                } else {
                    let result = if job.phase == Phase::FetchingForLoad {
                        history::bootstrap_step(&rpc, self.network, &binding, &path).await
                    } else {
                        history::recover_step(&rpc, self.network, &binding, &path).await
                    };
                    match result {
                        Ok(job) => status.job = Some(job),
                        Err(error) => {
                            status.error = Some(error);
                            status.job = history::read_json(&path)?;
                        }
                    }
                }
            }
        }
        if status
            .job
            .as_ref()
            .is_some_and(|job| matches!(job.phase, Phase::Complete | Phase::Reconciling))
        {
            let _lease = history::HistoryLease::acquire(&directory)?;
            let local_backend = matches!(config.bitcoin_backend.as_ref(), Some(BitcoinBackend::Bitcoind(active)) if active == &self.config);
            if local_backend && status.job.as_ref().is_some_and(|job| !job.db_replayed) {
                self.daemon
                    .replay_wallet_records()
                    .await
                    .map_err(|e| e.to_string())?;
                let mut saved: Recovery =
                    history::read_json(&path)?.ok_or("Missing recovery progress")?;
                saved.db_replayed = true;
                history::write_json(&path, &saved)?;
                status.job = Some(saved);
            }
            let coins = self
                .daemon
                .list_coins(
                    &[
                        CoinStatus::Unconfirmed,
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent,
                    ],
                    &[],
                )
                .await
                .map_err(|e| e.to_string())?
                .coins;
            status.reconciled =
                history::reconcile_coins(&rpc, &coins, history::outspend_source(config))
                    .await?
                    .is_none();
            if !status.reconciled {
                status.error=Some("The scan finished, but Connect and the local wallet still disagree about transaction/spend history. Sync Connect, then import or recover again before switching.".into());
                let mut saved: Recovery =
                    history::read_json(&path)?.ok_or("Missing recovery progress")?;
                saved.phase = Phase::Paused;
                // Resuming scans every block, the check single blocks missed.
                saved.rewind(Some(
                    "Checking single blocks left Connect and your node disagreeing, so COINCUBE Desktop scans every block."
                        .into(),
                ));
                saved.last_error = status.error.clone();
                history::write_json(&path, &saved)?;
                status.job = Some(saved);
            } else if status
                .job
                .as_ref()
                .is_some_and(|job| job.phase == Phase::Reconciling)
            {
                let mut saved: Recovery =
                    history::read_json(&path)?.ok_or("Missing recovery progress")?;
                if rpc.block_hash(saved.target).await? != saved.target_hash {
                    return Err("The chain changed during reconciliation; resume recovery".into());
                }
                saved.phase = Phase::Complete;
                saved.last_error = None;
                history::write_json(&path, &saved)?;
                status.job = Some(saved);
            }
            if status.reconciled
                && status.job.as_ref().is_some_and(|job| {
                    job.start == 0 && job.db_replayed && job.phase == Phase::Complete
                })
            {
                let checksum = self.wallet.descriptor_checksum.clone();
                super::settings::update_settings_file(
                    &self.datadir.network_directory(self.chain),
                    |settings| Some(super::cleared_pending_rescan(settings, &checksum)),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        // `finish_temporary` rewrites bitcoin.conf before the restart. Only
        // call it when its current policy could finish, with the same owner
        // guard held until the replacement node has claimed its lock.
        let mut finish_lifecycle = if status.policy.as_ref().is_some_and(|p| p.temporary)
            && retention::unfinished(&directory)?.is_empty()
        {
            // Another Cube may currently own the node. Keep this poll's status
            // and retry the finish after that owner releases it.
            match self.exclusive_lifecycle().await {
                Ok(lifecycle) => Some(lifecycle),
                Err(error) => {
                    record_temporary_finish_error(&mut status, error);
                    None
                }
            }
        } else {
            None
        };
        if finish_lifecycle.is_some() && retention::finish_temporary(&self.datadir, self.network)? {
            let conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(
                &internal_bitcoind_datadir(&self.datadir),
            ))
            .map_err(|e| e.to_string())?;
            let (cfg, started) =
                super::state::vault::settings::bitcoind::ensure_tor_and_start_managed_with_lifecycle(
                    self.datadir.clone(),
                    self.network,
                    conf.flavor,
                    None,
                    true,
                    None,
                    self.managed_owner.clone(),
                    finish_lifecycle.take(),
                )
                .await?;
            if cfg != self.config {
                return Err("Managed node endpoint changed; refresh node settings".into());
            }
            node = Some(started);
            status.policy = None;
        } else if status.policy.is_some() {
            drop(finish_lifecycle);
            if let Err(error) = retention::maintain(&self.datadir, self.network, &rpc, now()).await
            {
                status.error = Some(error);
            }
        }
        Ok(Outcome { status, node })
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_finish_defers_owner_refusal_without_losing_poll_status() {
        let mut status = Status {
            busy: true,
            reconciled: true,
            error: Some("Existing recovery warning".into()),
            ..Status::default()
        };
        record_temporary_finish_error(&mut status, OTHER_CUBE_OWNS_NODE.into());
        assert!(status.busy);
        assert!(status.reconciled);
        assert_eq!(status.error.as_deref(), Some("Existing recovery warning"));
    }

    #[test]
    fn temporary_finish_surfaces_unexpected_lifecycle_failure_in_status() {
        let mut status = Status {
            busy: true,
            ..Status::default()
        };
        record_temporary_finish_error(
            &mut status,
            "Could not enumerate managed-node owners".into(),
        );
        assert!(status.busy);
        assert_eq!(
            status.error.as_deref(),
            Some("Could not enumerate managed-node owners")
        );
    }

    #[test]
    fn start_accepts_exact_heights_and_calendar_dates() {
        assert!(matches!(
            parse_start("946453").unwrap(),
            Start::Height(946453)
        ));
        assert!(matches!(
            parse_start("2026-04-01").unwrap(),
            Start::Date(1775001600)
        ));
        for bad in ["-1", "2026-02-30", "04/01/26", "4294967296", "1969-01-01"] {
            assert!(parse_start(bad).is_err());
        }
    }
}
