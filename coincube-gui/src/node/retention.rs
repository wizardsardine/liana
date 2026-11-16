//! Managed rolling block retention. `prune=1` reserves deletion decisions for
//! this controller; block scanning and pruning share the same per-network lease.

use coincube_core::miniscript::bitcoin::{self, Network};
use coincubed::config::{BitcoindConfig, BitcoindRpcAuth};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    convert::TryFrom,
    path::{Path, PathBuf},
};

use super::{
    bitcoind::{
        internal_bitcoind_datadir, InternalBitcoindConfig, NodeChainFamily, PRUNE_DEFAULT,
        PRUNE_MIN,
    },
    history::{self, Binding, HistoryLease, NodeRpc, Phase, Recovery},
};
use crate::{chain::ChainId, dir::CoincubeDirectory};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub days: u32,
    pub previous_size_mb: u32,
    #[serde(default)]
    pub temporary: bool,
    pub keep_from: Option<u32>,
}

impl Policy {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || (!self.temporary && !(1..=3650).contains(&self.days))
            || (self.temporary && (self.days != 0 || self.keep_from.is_none()))
            || self.previous_size_mb < PRUNE_MIN
        {
            return Err("Invalid managed block retention policy".into());
        }
        Ok(())
    }
}

pub fn directory(datadir: &CoincubeDirectory, network: Network) -> PathBuf {
    datadir
        .path()
        .join("node-history")
        .join(network.to_string())
}

pub fn load(datadir: &CoincubeDirectory, network: Network) -> Result<Option<Policy>, String> {
    let policy: Option<Policy> =
        history::read_json(&directory(datadir, network).join("retention.json"))?;
    if let Some(policy) = &policy {
        policy.validate()?;
    }
    Ok(policy)
}

/// Only a COINCUBE-managed regular-Bitcoin node may have its pruning changed.
/// Validate ownership and chain family before even creating controller files.
pub fn is_managed_endpoint(
    datadir: &CoincubeDirectory,
    network: Network,
    chain: ChainId,
    cfg: &BitcoindConfig,
) -> bool {
    !chain.is_blake2b()
        && chain.bitcoin_network() == network
        && cfg.addr.ip().is_loopback()
        && cfg.rpc_auth
            == BitcoindRpcAuth::CookieFile(super::bitcoind::internal_bitcoind_cookie_path(
                &internal_bitcoind_datadir(datadir),
                &network,
            ))
}

pub fn managed_binding(
    datadir: &CoincubeDirectory,
    network: Network,
    chain: ChainId,
    cfg: &BitcoindConfig,
    wallet: &str,
    descriptor: &str,
) -> Result<Binding, String> {
    if chain.is_blake2b() || chain.bitcoin_network() != network {
        return Err("Wallet history recovery is available for regular Bitcoin only".into());
    }
    let root = internal_bitcoind_datadir(datadir);
    let BitcoindRpcAuth::CookieFile(cookie) = &cfg.rpc_auth else {
        return Err("Managed recovery requires the managed node's cookie authentication".into());
    };
    if !is_managed_endpoint(datadir, network, chain, cfg) {
        return Err("This node is not managed by COINCUBE Desktop".into());
    }
    let conf =
        InternalBitcoindConfig::from_file(&root.join("bitcoin.conf")).map_err(|e| e.to_string())?;
    let section = conf
        .networks
        .get(&network)
        .ok_or("No managed node configured for this network")?;
    if section.rpc_port != cfg.addr.port() || conf.flavor.chain_family() != NodeChainFamily::Bitcoin
    {
        return Err("The configured endpoint is not this managed Bitcoin node".into());
    }
    let node_instance =
        super::bitcoind::ensure_node_instance_marker(cookie).map_err(|e| e.to_string())?;
    Ok(Binding {
        genesis: bitcoin::blockdata::constants::genesis_block(network).block_hash(),
        node_instance,
        endpoint: cfg.addr.to_string(),
        wallet: wallet.into(),
        descriptor: descriptor.into(),
    })
}

/// Persist intent before switching pruning mode. If interrupted between the
/// two atomic files, recovery refuses automatic pruning until restart repairs
/// the conf. The lease-to-conf order is always one-way, with no RPC under conf.
pub fn configure(
    datadir: &CoincubeDirectory,
    network: Network,
    chain: ChainId,
    cfg: &BitcoindConfig,
    days: u32,
) -> Result<Policy, String> {
    configure_mode(datadir, network, chain, cfg, days, false, None)
}

pub fn configure_recovery(
    datadir: &CoincubeDirectory,
    network: Network,
    chain: ChainId,
    cfg: &BitcoindConfig,
    prune_height: u32,
) -> Result<Policy, String> {
    configure_mode(datadir, network, chain, cfg, 0, true, Some(prune_height))
}

fn configure_mode(
    datadir: &CoincubeDirectory,
    network: Network,
    chain: ChainId,
    cfg: &BitcoindConfig,
    days: u32,
    temporary: bool,
    keep_from: Option<u32>,
) -> Result<Policy, String> {
    managed_binding(datadir, network, chain, cfg, "", "")?;
    let dir = directory(datadir, network);
    let _lease = HistoryLease::acquire(&dir)?;
    let previous = load(datadir, network)?;
    let (days, temporary, keep_from) = if temporary {
        previous
            .as_ref()
            .map(|policy| (policy.days, policy.temporary, policy.keep_from))
            .unwrap_or((days, temporary, keep_from))
    } else {
        (days, temporary, keep_from)
    };
    let mut policy = Policy {
        version: 1,
        days,
        previous_size_mb: previous
            .as_ref()
            .map(|p| p.previous_size_mb)
            .unwrap_or(PRUNE_DEFAULT),
        temporary,
        keep_from,
    };
    policy.validate()?;
    super::managed_conf::update_managed_conf(datadir, NodeChainFamily::Bitcoin, |txn| {
        let mut conf = txn.conf.clone().ok_or_else(|| {
            super::managed_conf::ManagedConfEditError::Other(
                "Managed node configuration is missing".into(),
            )
        })?;
        let section = conf.networks.get_mut(&network).ok_or_else(|| {
            super::managed_conf::ManagedConfEditError::Other(
                "Managed network configuration is missing".into(),
            )
        })?;
        if previous.is_none() && section.prune >= PRUNE_MIN {
            policy.previous_size_mb = section.prune;
        }
        history::write_json(&dir.join("retention.json"), &policy)
            .map_err(super::managed_conf::ManagedConfEditError::Other)?;
        section.prune = 1;
        Ok(((), Some(conf)))
    })
    .map_err(|e| e.to_string())?;
    Ok(policy)
}

/// Conservative date resolution using monotonic median times, a two-hour
/// timestamp allowance and a day's block margin. Direct heights stay exact.
pub async fn height_from_date(rpc: &NodeRpc, timestamp: u64, tip: u32) -> Result<u32, String> {
    let cutoff = timestamp.saturating_sub(7200);
    let (mut low, mut high) = (0, tip);
    while low < high {
        let mid = low + (high - low) / 2;
        let hash = rpc.block_hash(mid).await?;
        let header = rpc
            .call(false, "getblockheader", json!([hash.to_string()]))
            .await
            .map_err(|e| e.to_string())?;
        let time = header["mediantime"]
            .as_u64()
            .ok_or("Node did not report the block median time")?;
        if time < cutoff {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    Ok(low.saturating_sub(144))
}

pub fn unfinished(directory: &Path) -> Result<Vec<Recovery>, String> {
    let mut jobs = vec![];
    match std::fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json")
                    || path.file_name().and_then(|s| s.to_str()) == Some("retention.json")
                {
                    continue;
                }
                let job: Recovery =
                    history::read_json(&path)?.ok_or("Recovery file disappeared")?;
                job.validate(&job.binding)?;
                if !matches!(job.phase, Phase::Complete | Phase::Cancelled) {
                    jobs.push(job);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(jobs)
}

/// A cancelled bootstrap still has an unloaded wallet which Core must not
/// autoload before the node can serve recovery blocks. Its database is intact.
pub fn requires_wallet_bootstrap(
    datadir: &CoincubeDirectory,
    network: Network,
) -> Result<bool, String> {
    let dir = directory(datadir, network);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json")
            || path.file_name().and_then(|value| value.to_str()) == Some("retention.json")
        {
            continue;
        }
        let job: Recovery = history::read_json(&path)?.ok_or("Missing recovery progress")?;
        job.validate(&job.binding)?;
        if !job.wallet_ready {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Delete only eligible old files. An unfinished range caps deletion at its
/// last persisted scanned block, including jobs paused in another Cube.
pub async fn maintain(
    datadir: &CoincubeDirectory,
    network: Network,
    rpc: &NodeRpc,
    now: u64,
) -> Result<Option<u32>, String> {
    let dir = directory(datadir, network);
    let _lease = HistoryLease::acquire(&dir)?;
    let Some(policy) = load(datadir, network)? else {
        return Ok(None);
    };
    let info = rpc.check_chain(network).await?;
    if info["pruned"].as_bool() != Some(true) || info["automatic_pruning"].as_bool() != Some(false)
    {
        return Err("Restart the managed node to apply rolling retention".into());
    }
    let cutoff = now.saturating_sub(u64::from(policy.days) * 86400);
    let jobs = unfinished(&dir)?;
    if policy.temporary {
        let Some(limit) = jobs
            .iter()
            .map(scanned_pruning_limit)
            .collect::<Option<Vec<_>>>()
            .and_then(|heights| heights.into_iter().min())
        else {
            return Ok(None);
        };
        let Some(keep) = policy.keep_from.and_then(|height| height.checked_sub(1)) else {
            return Ok(None);
        };
        return rpc
            .call(false, "pruneblockchain", json!([limit.min(keep)]))
            .await
            .map_err(|e| e.to_string())?
            .as_u64()
            .map(|height| {
                u32::try_from(height)
                    .map(Some)
                    .map_err(|_| "Pruning height is out of range".into())
            })
            .ok_or("Invalid pruning result")?;
    }
    let argument = if jobs.is_empty() {
        // Core's timestamp pruning uses the chain's maximum ancestor times,
        // preserving a conservative boundary even when timestamps go backwards.
        cutoff
    } else {
        let Some(limit) = jobs
            .iter()
            .map(scanned_pruning_limit)
            .collect::<Option<Vec<_>>>()
            .and_then(|v| v.into_iter().min())
        else {
            return Ok(None);
        };
        let hash = rpc.block_hash(limit).await?;
        let header = rpc
            .call(false, "getblockheader", json!([hash.to_string()]))
            .await
            .map_err(|e| e.to_string())?;
        // This block itself is at/after this timestamp. Core's earliest-
        // ancestor timestamp search therefore cannot choose a deletion boundary
        // beyond the persisted scan, even with non-monotonic block times.
        cutoff.min(
            header["time"]
                .as_u64()
                .ok_or("Missing scanned block time")?,
        )
    };
    // Time-based RPC arguments must exceed Core's height/timestamp discriminator.
    if argument <= 1_000_000_000 {
        return Ok(None);
    }
    let result = rpc
        .call(false, "pruneblockchain", json!([argument]))
        .await
        .map_err(|e| e.to_string())?;
    let height = result.as_u64().ok_or("Invalid pruning result")?;
    Ok(Some(
        u32::try_from(height).map_err(|_| "Pruning height is out of range")?,
    ))
}

/// Resource-size editing explicitly leaves rolling mode, and is refused while
/// a recovery needs blocks. Conf writes still use the existing restart path.
pub fn disable(datadir: &CoincubeDirectory, network: Network) -> Result<(), String> {
    let dir = directory(datadir, network);
    let _lease = HistoryLease::acquire(&dir)?;
    if !unfinished(&dir)?.is_empty() {
        return Err(
            "Finish or cancel wallet recovery before changing the node's storage target".into(),
        );
    }
    match std::fs::remove_file(dir.join("retention.json")) {
        Ok(()) => sync_directory(&dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn scanned_pruning_limit(job: &Recovery) -> Option<u32> {
    if job.phase == Phase::Reconciling || job.next > job.target {
        job.start.checked_sub(1)
    } else {
        job.next.checked_sub(1)
    }
}

/// Restore the user's storage-size target after temporary recovery. Persist
/// the conf before removing intent, so interruption can only retain more data.
pub fn finish_temporary(datadir: &CoincubeDirectory, network: Network) -> Result<bool, String> {
    let dir = directory(datadir, network);
    let _lease = HistoryLease::acquire(&dir)?;
    let Some(policy) = load(datadir, network)? else {
        return Ok(false);
    };
    if !policy.temporary || !unfinished(&dir)?.is_empty() {
        return Ok(false);
    }
    super::managed_conf::update_managed_conf(datadir, NodeChainFamily::Bitcoin, |txn| {
        let mut conf = txn.conf.clone().ok_or_else(|| {
            super::managed_conf::ManagedConfEditError::Other(
                "Missing managed node configuration".into(),
            )
        })?;
        let section = conf.networks.get_mut(&network).ok_or_else(|| {
            super::managed_conf::ManagedConfEditError::Other(
                "Missing managed network configuration".into(),
            )
        })?;
        section.prune = policy.previous_size_mb;
        Ok(((), Some(conf)))
    })
    .map_err(|e| e.to_string())?;
    std::fs::remove_file(dir.join("retention.json")).map_err(|e| e.to_string())?;
    sync_directory(&dir)?;
    Ok(true)
}

fn sync_directory(directory: &Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::hashes::Hash;
    #[test]
    fn downloading_and_reconciling_never_publish_unscanned_blocks_for_pruning() {
        let binding = Binding {
            genesis: bitcoin::BlockHash::all_zeros(),
            node_instance: "fixture".into(),
            endpoint: "fixture".into(),
            wallet: "fixture".into(),
            descriptor: "fixture".into(),
        };
        let mut job = Recovery::new(binding, 100, 1000, bitcoin::BlockHash::all_zeros()).unwrap();
        job.phase = Phase::FetchingForLoad;
        job.download_next = 800;
        assert_eq!(scanned_pruning_limit(&job), Some(99));
        job.phase = Phase::Paused;
        job.next = 132;
        assert_eq!(scanned_pruning_limit(&job), Some(131));
        job.phase = Phase::Reconciling;
        job.next = 1001;
        assert_eq!(scanned_pruning_limit(&job), Some(99));
        job.phase = Phase::Paused;
        assert_eq!(scanned_pruning_limit(&job), Some(99));
        job.start = 0;
        assert_eq!(scanned_pruning_limit(&job), None);
    }
}
