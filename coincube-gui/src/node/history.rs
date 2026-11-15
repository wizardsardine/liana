//! Verified transaction handoff and resumable historical wallet scans.
//!
//! Recovery keeps the node's active chainstate. Block bodies are fetched and
//! scanned in bounded ranges; undo data is not required by a wallet rescan.
//! Automatic pruning must be disabled before fetching any missing block.

use std::{
    collections::{BTreeMap, BTreeSet},
    convert::TryFrom,
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{
        self,
        consensus::encode::{deserialize, serialize_hex},
        hashes::{sha256, Hash},
        BlockHash, MerkleBlock, Network, OutPoint, Txid,
    },
};
use coincubed::config::{BitcoindConfig, EsploraConfig};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const RESPONSE_LIMIT: usize = 10 * 1024 * 1024;
pub const BATCH_BLOCKS: u32 = 32;
const MIN_FREE_BYTES: u64 = 512 * 1024 * 1024;
/// About a minute of 5-second polls. A node busy with a rescan or a peer
/// download can miss one RPC deadline without anything being wrong.
pub const MAX_TRANSIENT_FAILURES: u32 = 12;
const UNREACHABLE: &str = "Cannot reach local node for ";

fn chain_changed(start: u32) -> String {
    format!(
        "The chain changed during recovery, so COINCUBE Desktop scans every block from {start}."
    )
}

/// Keep running through a missed RPC deadline; pause on anything else, or
/// once the node has been unreachable for too long. Returns whether to pause.
fn absorb_transient(job: &mut Recovery, error: &str) -> bool {
    if error.starts_with(UNREACHABLE) && job.transient_failures < MAX_TRANSIENT_FAILURES {
        job.transient_failures += 1;
        job.last_error = Some(format!("{error}. Retrying…"));
        return false;
    }
    job.transient_failures = 0;
    job.phase = Phase::Paused;
    job.last_error = Some(error.to_string());
    true
}

/// A transport/RPC failure never means that a transaction or block is absent.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: Option<i64>,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<String> for RpcError {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

/// Secrets are deliberately excluded from Debug and persisted job records.
pub struct NodeRpc {
    client: reqwest::Client,
    node_url: reqwest::Url,
    wallet_url: reqwest::Url,
    user: String,
    pass: String,
}

impl NodeRpc {
    pub async fn new(
        cfg: &BitcoindConfig,
        wallet: &str,
        chain: crate::chain::ChainId,
    ) -> Result<Self, String> {
        if chain.is_blake2b() {
            return Err("History recovery is available for regular Bitcoin only".into());
        }
        let (user, pass) = crate::app::local_switch::rpc_credentials(cfg).await?;
        let node_url =
            reqwest::Url::parse(&format!("http://{}/", cfg.addr)).map_err(|e| e.to_string())?;
        let mut wallet_url = node_url.clone();
        wallet_url
            .path_segments_mut()
            .map_err(|_| "Invalid node URL")?
            .clear()
            .push("wallet")
            .push(wallet);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            node_url,
            wallet_url,
            user,
            pass,
        })
    }

    pub async fn call(&self, wallet: bool, method: &str, params: Value) -> Result<Value, RpcError> {
        let url = if wallet {
            &self.wallet_url
        } else {
            &self.node_url
        };
        let response = self
            .client
            .post(url.clone())
            .basic_auth(&self.user, Some(&self.pass))
            .timeout(Duration::from_secs(
                if matches!(method, "rescanblockchain" | "loadwallet") {
                    120
                } else {
                    10
                },
            ))
            .json(&json!({"jsonrpc":"2.0", "id":"history", "method":method, "params":params}))
            .send()
            .await
            .map_err(|_| format!("Cannot reach local node for {method}"))?;
        let bytes = bounded_body(response, RESPONSE_LIMIT).await?;
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid node response".to_string())?;
        if response["id"].as_str() != Some("history") {
            return Err("Mismatched node response".to_string().into());
        }
        if !response["error"].is_null() {
            return Err(RpcError {
                code: response["error"]["code"].as_i64(),
                message: format!(
                    "Local node refused {method}: {}",
                    response["error"]["message"].as_str().unwrap_or("RPC error")
                ),
            });
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "Missing node result".to_string().into())
    }

    pub async fn block_hash(&self, height: u32) -> Result<BlockHash, String> {
        self.call(false, "getblockhash", json!([height]))
            .await
            .map_err(|e| e.to_string())?
            .as_str()
            .ok_or("Missing block hash")?
            .parse()
            .map_err(|_| "Invalid block hash".into())
    }

    pub async fn check_chain(&self, network: Network) -> Result<Value, String> {
        if self.block_hash(0).await?
            != bitcoin::blockdata::constants::genesis_block(network).block_hash()
        {
            return Err("The local node serves a different Bitcoin network".into());
        }
        let info = self
            .call(false, "getblockchaininfo", json!([]))
            .await
            .map_err(|e| e.to_string())?;
        if info["initialblockdownload"].as_bool() != Some(false) {
            return Err("Wait for the local node to finish blockchain sync".into());
        }
        Ok(info)
    }

    pub async fn knows_transaction(&self, txid: Txid) -> Result<bool, String> {
        match self
            .call(true, "gettransaction", json!([txid.to_string(), true]))
            .await
        {
            Ok(tx) if tx["txid"].as_str() == Some(txid.to_string().as_str()) => Ok(true),
            Ok(_) => Err("Invalid wallet transaction response".into()),
            Err(e) if e.code == Some(-5) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn transaction_at(
        &self,
        txid: Txid,
        height: Option<u32>,
    ) -> Result<Option<bitcoin::Transaction>, String> {
        let value = match self
            .call(true, "gettransaction", json!([txid.to_string(), true]))
            .await
        {
            Ok(value) => value,
            Err(error) if error.code == Some(-5) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let raw = value["hex"]
            .as_str()
            .ok_or("Wallet did not return the raw transaction")?;
        let bytes = <Vec<u8> as bitcoin::hex::FromHex>::from_hex(raw)
            .map_err(|_| "Invalid wallet transaction hex")?;
        let tx: bitcoin::Transaction =
            deserialize(&bytes).map_err(|_| "Invalid wallet transaction")?;
        if tx.compute_txid() != txid {
            return Err("Wallet returned a different transaction".into());
        }
        if let Some(height) = height {
            if value["confirmations"].as_i64().is_none_or(|n| n <= 0)
                || value["blockhash"].as_str()
                    != Some(self.block_hash(height).await?.to_string().as_str())
            {
                return Ok(None);
            }
        } else if value["confirmations"].as_i64() != Some(0) {
            return Ok(None);
        } else {
            match self
                .call(false, "getmempoolentry", json!([txid.to_string()]))
                .await
            {
                Ok(entry) if entry.is_object() => {}
                Ok(_) => return Err("Invalid mempool transaction response".into()),
                Err(error) if error.code == Some(-5) => return Ok(None),
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(Some(tx))
    }

    /// An unloaded stale wallet can require block bodies before Core permits
    /// attachment. Only an explicit pruning refusal enters bootstrap mode.
    pub async fn ensure_loaded(&self, wallet: &str) -> Result<bool, String> {
        let loaded = self
            .call(false, "listwallets", json!([]))
            .await
            .map_err(|e| e.to_string())?;
        let loaded = loaded.as_array().ok_or("Cannot read loaded wallets")?;
        if loaded.iter().any(|name| name.as_str() == Some(wallet)) {
            return Ok(true);
        }
        if Path::new(wallet).exists() {
            match self.call(false, "loadwallet", json!([wallet])).await {
                Ok(_) => Ok(true),
                Err(error)
                    if error.code == Some(-4)
                        && error.message.to_ascii_lowercase().contains("prun") =>
                {
                    Ok(false)
                }
                Err(error) => Err(error.to_string()),
            }
        } else {
            self.call(
                false,
                "createwallet",
                json!([wallet, true, true, "", false, true, true]),
            )
            .await
            .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }

    /// Import descriptors with an adequate range before scanning/importing funds.
    /// Existing wallets must be watch-only descriptor wallets with these same
    /// descriptors; never populate an unrelated wallet at the requested path.
    pub async fn prepare_wallet(
        &self,
        wallet: &str,
        descriptor: &CoincubeDescriptor,
        range: u32,
    ) -> Result<(), String> {
        if !self.ensure_loaded(wallet).await? {
            return Err("Download the wallet's catch-up blocks before loading it".into());
        }
        let info = self
            .call(true, "getwalletinfo", json!([]))
            .await
            .map_err(|e| e.to_string())?;
        if info["private_keys_enabled"].as_bool() != Some(false)
            || info["descriptors"].as_bool() != Some(true)
        {
            return Err("Recovery requires COINCUBE Desktop's watch-only descriptor wallet".into());
        }
        let receive = descriptor.receive_descriptor().to_string();
        let change = descriptor.change_descriptor().to_string();
        let existing = self
            .call(true, "listdescriptors", json!([]))
            .await
            .map_err(|e| e.to_string())?;
        let entries = existing["descriptors"]
            .as_array()
            .ok_or("Cannot read wallet descriptors")?;
        if entries
            .iter()
            .any(|d| !matches!(d["desc"].as_str(), Some(s) if s == receive || s == change))
        {
            return Err("The local wallet contains different descriptors".into());
        }
        let range = entries
            .iter()
            .filter_map(|entry| entry["range"][1].as_u64())
            .max()
            .unwrap_or(0)
            .max(u64::from(range));
        let result = self.call(true, "importdescriptors", json!([[{"desc":receive,"timestamp":"now","range":[0,range]}, {"desc":change,"timestamp":"now","range":[0,range]}]])).await.map_err(|e| e.to_string())?;
        if !result
            .as_array()
            .is_some_and(|a| a.len() == 2 && a.iter().all(|r| r["success"].as_bool() == Some(true)))
        {
            return Err("Could not prepare the wallet descriptors for recovery".into());
        }
        Ok(())
    }

    /// Core's pruneheight includes undo availability. Wallet rescans only need
    /// block bodies, so query actual bodies instead of using that height.
    async fn has_block(&self, hash: BlockHash) -> Result<bool, String> {
        match self
            .call(false, "getblock", json!([hash.to_string(), 0]))
            .await
        {
            Ok(raw) => {
                let hex = raw.as_str().ok_or("Invalid block body response")?;
                let bytes = <Vec<u8> as bitcoin::hex::FromHex>::from_hex(hex)
                    .map_err(|_| "Invalid block body hex")?;
                let block: bitcoin::Block =
                    deserialize(&bytes).map_err(|_| "Invalid block body")?;
                if block.block_hash() != hash
                    || !block.check_merkle_root()
                    || !block.check_witness_commitment()
                {
                    return Err("The downloaded block does not match the local chain".into());
                }
                Ok(true)
            }
            Err(e) if e.code == Some(-1) && e.message.contains("pruned") => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn fetch_block(&self, hash: BlockHash, attempt: u32) -> Result<bool, String> {
        if self.has_block(hash).await? {
            return Ok(true);
        }
        let peers = self
            .call(false, "getpeerinfo", json!([]))
            .await
            .map_err(|e| e.to_string())?;
        let peers = peers.as_array().ok_or("Cannot read node peers")?;
        let mut ids = Vec::new();
        for peer in peers
            .iter()
            .filter(|p| p["inbound"].as_bool() == Some(false))
        {
            let Some(services) = peer["services"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
            else {
                continue;
            };
            if services & 9 != 9 {
                continue;
            } // NODE_NETWORK + NODE_WITNESS
            let Some(id) = peer["id"].as_i64() else {
                continue;
            };
            ids.push(id);
        }
        let mut scheduled = false;
        if !ids.is_empty() {
            let offset = attempt as usize % ids.len();
            ids.rotate_left(offset);
        }
        for id in ids {
            if self
                .call(false, "getblockfrompeer", json!([hash.to_string(), id]))
                .await
                .is_ok()
            {
                scheduled = true;
                break;
            }
        }
        if !scheduled {
            return Err("No archival peer is available to recover old blocks; retry after the node connects to peers".into());
        }
        // Scheduling is not receipt. Allow a bounded download window before
        // consuming a retry or selecting another peer for the same block.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if self.has_block(hash).await? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
        }
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err("Node/provider response is too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cannot read node/provider response")?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Node/provider response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// An inclusion proof is verified before being sent to the wallet. Local
/// getblockhash supplies the active-chain authority; provider metadata does not.
pub fn verify_proof(raw: &[u8], txid: Txid, block: BlockHash) -> Result<MerkleBlock, String> {
    let proof: MerkleBlock = deserialize(raw).map_err(|_| "Invalid transaction inclusion proof")?;
    let mut matches = vec![];
    let mut indexes = vec![];
    let root = proof
        .txn
        .extract_matches(&mut matches, &mut indexes)
        .map_err(|_| "Invalid proof merkle tree")?;
    if proof.header.block_hash() != block
        || root != proof.header.merkle_root
        || !matches.contains(&txid)
    {
        return Err("Transaction proof does not match the local active chain".into());
    }
    Ok(proof)
}

fn providers(config: &EsploraConfig) -> impl Iterator<Item = (&String, Option<&String>)> {
    IntoIterator::into_iter([
        Some((&config.addr, config.token.as_ref())),
        config
            .fallback_addr
            .as_ref()
            .map(|a| (a, config.fallback_token.as_ref())),
        config
            .secondary_fallback_addr
            .as_ref()
            .map(|a| (a, config.secondary_fallback_token.as_ref())),
    ])
    .flatten()
}

/// The Connect/Esplora source the wallet engine uses, else its fallback.
pub fn outspend_source(config: &coincubed::config::Config) -> Option<&EsploraConfig> {
    match config.bitcoin_backend.as_ref() {
        Some(coincubed::config::BitcoinBackend::Esplora(source)) => Some(source),
        _ => config.fallback_esplora.as_ref(),
    }
}

/// Ask Connect/Esplora which confirmed transaction and block spent
/// `outpoint`. The caller checks the block hash against the local chain, and
/// only the local wallet's copy of the spender proves the spend; a wrong
/// answer only costs a scan.
pub async fn fetch_outspend(
    config: &EsploraConfig,
    outpoint: OutPoint,
) -> Result<Option<(Txid, u32, BlockHash)>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    for (base, token) in providers(config) {
        let mut request = client.get(format!(
            "{}/tx/{}/outspend/{}",
            base.trim_end_matches('/'),
            outpoint.txid,
            outpoint.vout
        ));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let Ok(response) = request.send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(raw) = bounded_body(response, 64 * 1024).await else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        if value["spent"].as_bool() != Some(true)
            || value["status"]["confirmed"].as_bool() != Some(true)
        {
            return Ok(None);
        }
        let spender = value["txid"].as_str().and_then(|t| t.parse::<Txid>().ok());
        let height = value["status"]["block_height"]
            .as_u64()
            .and_then(|h| u32::try_from(h).ok());
        let hash = value["status"]["block_hash"]
            .as_str()
            .and_then(|h| h.parse::<BlockHash>().ok());
        if let (Some(spender), Some(height), Some(hash)) = (spender, height, hash) {
            return Ok(Some((spender, height, hash)));
        }
    }
    Err("No configured Connect/Esplora provider said which transaction spent this coin".into())
}

pub async fn fetch_proof(
    config: &EsploraConfig,
    txid: Txid,
    block: BlockHash,
) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    for (base, token) in providers(config) {
        let mut request = client.get(format!(
            "{}/tx/{txid}/merkleblock-proof",
            base.trim_end_matches('/')
        ));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let Ok(response) = request.send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(raw) = bounded_body(response, 1024 * 1024).await else {
            continue;
        };
        let Ok(hex) = std::str::from_utf8(&raw) else {
            continue;
        };
        if let Ok(bytes) = <Vec<u8> as bitcoin::hex::FromHex>::from_hex(hex.trim()) {
            if verify_proof(&bytes, txid, block).is_ok() {
                return Ok(bytes);
            }
        }
    }
    Err("No configured Connect/Esplora provider supplied an inclusion proof; recover this transaction's block instead".into())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub genesis: BlockHash,
    pub node_instance: String,
    pub endpoint: String,
    pub wallet: String,
    pub descriptor: String,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("genesis", &self.genesis)
            .field("node_instance", &self.node_instance)
            .field("endpoint", &self.endpoint)
            .field("wallet", &self.wallet)
            .field("descriptor", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    FetchingForLoad,
    Downloading,
    Scanning,
    Reconciling,
    Paused,
    Complete,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub version: u32,
    pub id: String,
    pub binding: Binding,
    pub start: u32,
    pub next: u32,
    pub target: u32,
    pub target_hash: BlockHash,
    pub phase: Phase,
    pub fetch_attempts: u32,
    pub last_error: Option<String>,
    pub download_next: u32,
    pub wallet_ready: bool,
    pub import_requested: bool,
    #[serde(default)]
    pub db_replayed: bool,
    /// Single blocks still to fetch and scan, ascending and below `tail`.
    /// While any remain, `next` is the first so pruning keeps every one.
    #[serde(default)]
    pub blocks: Vec<u32>,
    /// Start of the contiguous range scanned after `blocks`. `None` is a
    /// plain range scan from `start`.
    #[serde(default)]
    pub tail: Option<u32>,
    /// How many single blocks were planned, for progress.
    #[serde(default)]
    pub planned: u32,
    /// Why this recovery scans a long contiguous range instead of single
    /// blocks, shown to the user.
    #[serde(default)]
    pub fallback: Option<String>,
    /// Consecutive node-unreachable steps. The job keeps running until
    /// `MAX_TRANSIENT_FAILURES`, then pauses.
    #[serde(default)]
    pub transient_failures: u32,
    /// The requested range's start while `plan` has narrowed `start`; a
    /// rewind scans from here.
    #[serde(default)]
    pub full_start: Option<u32>,
}

/// What a set of coins needs scanned so the local wallet agrees with Connect.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanPlan {
    /// Blocks that each hold a transaction the wallet is missing.
    pub blocks: BTreeSet<u32>,
    /// A height from which every block must be scanned, and why.
    pub floor: Option<(u32, String)>,
}

impl ScanPlan {
    pub fn earliest(&self) -> Option<u32> {
        self.blocks
            .first()
            .copied()
            .into_iter()
            .chain(self.floor.as_ref().map(|(height, _)| *height))
            .min()
    }
    fn lower_floor(&mut self, height: u32, reason: String) {
        if self.floor.as_ref().is_none_or(|(floor, _)| height < *floor) {
            self.floor = Some((height, reason));
        }
    }
}

impl Recovery {
    pub fn new(
        binding: Binding,
        start: u32,
        target: u32,
        target_hash: BlockHash,
    ) -> Result<Self, String> {
        if start > target || target == u32::MAX {
            return Err("Recovery height must be at or below the current chain tip".into());
        }
        Ok(Self {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            binding,
            start,
            next: start,
            target,
            target_hash,
            phase: Phase::Downloading,
            fetch_attempts: 0,
            last_error: None,
            download_next: start,
            wallet_ready: false,
            import_requested: false,
            db_replayed: false,
            blocks: Vec::new(),
            tail: None,
            planned: 0,
            fallback: None,
            transient_failures: 0,
            full_start: None,
        })
    }

    /// Replace the range with single blocks plus a contiguous tail. Blocks at
    /// or above the tail are covered by it. `tail` must not exceed `target`.
    pub fn plan(&mut self, blocks: BTreeSet<u32>, tail: u32, fallback: Option<String>) {
        let tail = tail.min(self.target);
        self.blocks = blocks.into_iter().filter(|h| *h < tail).collect();
        self.tail = Some(tail);
        self.planned = self.blocks.len() as u32;
        self.fallback = fallback;
        let full = self.range_start();
        self.start = self.blocks.first().copied().unwrap_or(tail);
        // A tail below the oldest coin widens the range; keep the lower start.
        self.full_start = Some(full.min(self.start));
        self.next = self.start;
        self.download_next = self.start;
    }

    /// The start of the range the user asked for, before any `plan`, or the
    /// plan's start when its tail reaches lower.
    pub fn range_start(&self) -> u32 {
        self.full_start.unwrap_or(self.start)
    }

    /// Drop any single-block plan and scan every block of the requested
    /// range again.
    pub fn rewind(&mut self, reason: Option<String>) {
        self.start = self.range_start();
        self.full_start = None;
        self.next = self.start;
        self.download_next = self.start;
        self.db_replayed = false;
        if self.tail.is_some() {
            self.blocks.clear();
            self.tail = None;
            self.planned = 0;
            self.fallback = reason;
        }
    }
    pub fn validate(&self, binding: &Binding) -> Result<(), String> {
        if self.version != 1
            || &self.binding != binding
            || self.start > self.next
            || self.next > self.target.saturating_add(1)
            || self.target == u32::MAX
            || self.start > self.target
            || (self.phase == Phase::Complete && self.next != self.target + 1)
            || self.download_next < self.start
            || self.download_next > self.target.saturating_add(1)
            || !self.blocks.windows(2).all(|w| w[0] < w[1])
            || self.blocks.len() as u64 > u64::from(self.planned)
            || self
                .full_start
                .is_some_and(|full| self.tail.is_none() || full > self.start)
            || match self.tail {
                None => !self.blocks.is_empty(),
                Some(tail) => {
                    tail < self.start
                        || tail > self.target
                        || self.blocks.first().is_some_and(|h| *h != self.next)
                        || self.blocks.iter().any(|h| *h < self.start || *h >= tail)
                        || (self.blocks.is_empty() && self.next < tail)
                }
            }
        {
            return Err(
                "Recovery progress belongs to a different wallet/node or is invalid".into(),
            );
        }
        Ok(())
    }
    pub fn progress(&self) -> f32 {
        let (done, total) = self.block_counts();
        done as f32 / total as f32
    }

    /// Blocks scanned and blocks to scan in all, counting single blocks and
    /// the contiguous tail alike.
    pub fn block_counts(&self) -> (u32, u32) {
        match self.tail {
            None => (self.next - self.start, self.target - self.start + 1),
            Some(tail) => {
                let singles_done = self.planned - self.blocks.len() as u32;
                let tail_done = if self.blocks.is_empty() {
                    self.next - tail
                } else {
                    0
                };
                (
                    singles_done + tail_done,
                    self.planned + (self.target - tail + 1),
                )
            }
        }
    }
}

/// Per-network controller lock: recovery and pruning cannot run concurrently,
/// even across multiple Cubes, windows or processes.
pub struct HistoryLease(File);
impl HistoryLease {
    pub fn acquire(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("controller.lock"))
            .map_err(|e| e.to_string())?;
        if !file.try_lock_exclusive().map_err(|e| e.to_string())? {
            return Err(
                "Another wallet is recovering or pruning this node; retry when it finishes".into(),
            );
        }
        Ok(Self(file))
    }
}
impl Drop for HistoryLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.len() > 1024 * 1024 {
        return Err("History progress file is too large".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::node::managed_conf::write_conf_atomically(path, &bytes).map_err(|e| e.to_string())
}

pub fn recovery_path(directory: &Path, wallet: &str) -> PathBuf {
    directory.join(format!("{}.json", sha256::Hash::hash(wallet.as_bytes())))
}

/// A persisted checkpoint advances only after an acknowledged, successful
/// bounded scan. RPC timeouts leave the batch repeatable; they never mean done.
pub async fn bootstrap_step(
    rpc: &NodeRpc,
    network: Network,
    binding: &Binding,
    path: &Path,
) -> Result<Recovery, String> {
    let dir = path.parent().ok_or("Missing recovery directory")?;
    let _lease = HistoryLease::acquire(dir)?;
    let mut job: Recovery = read_json(path)?.ok_or("Missing recovery progress")?;
    job.validate(binding)?;
    let before = job.download_next;
    let result:Result<(),String>=async {
        let info=rpc.check_chain(network).await?;
        if info["automatic_pruning"].as_bool()!=Some(false) || info["pruned"].as_bool()!=Some(true) {return Err("Restart the managed node with recovery pruning controls".into());}
        let anchor=rpc.block_hash(job.target).await?;
        if anchor!=job.target_hash {job.rewind(Some(chain_changed(job.range_start())));job.target_hash=anchor;}
        let end=job.download_next.saturating_add(BATCH_BLOCKS-1).min(job.target);
        for height in job.download_next..=end {
            if fs4::available_space(dir).map_err(|e|e.to_string())?<MIN_FREE_BYTES {return Err("Free more disk space to download the blocks needed to load this wallet, then resume".into());}
            if !rpc.fetch_block(rpc.block_hash(height).await?,job.fetch_attempts).await? {
                job.fetch_attempts+=1;
                if job.fetch_attempts>=8 {return Err("An archival peer did not supply the wallet's catch-up block; retry recovery".into());}
                return Ok(());
            }
            job.download_next=height+1;
            job.fetch_attempts=0;
            write_json(path,&job)?;
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        let paused = absorb_transient(&mut job, &error);
        write_json(path, &job)?;
        return if paused { Err(error) } else { Ok(job) };
    }
    // Only a step that fetched something ends a run of node failures.
    if job.transient_failures > 0 && job.download_next != before {
        job.transient_failures = 0;
        job.last_error = None;
    }
    write_json(path, &job)?;
    Ok(job)
}

pub async fn recover_step(
    rpc: &NodeRpc,
    network: Network,
    binding: &Binding,
    path: &Path,
) -> Result<Recovery, String> {
    let directory = path.parent().ok_or("Missing recovery directory")?;
    let _lease = HistoryLease::acquire(directory)?;
    let mut recovery: Recovery = read_json(path)?.ok_or("No saved wallet recovery to resume")?;
    recovery.validate(binding)?;
    if matches!(recovery.phase, Phase::Paused | Phase::Cancelled) {
        return Ok(recovery);
    }
    let before = (recovery.next, recovery.blocks.len());
    let result: Result<(),String> = async {
        let info = rpc.check_chain(network).await?;
        if rpc.block_hash(recovery.target).await? != recovery.target_hash {
            // Every previously scanned block may be on the old branch. Rewind
            // the entire requested range; stale checkpoints cannot authorize a
            // switch. Keep the user's range and preserve wallet records.
            recovery.rewind(Some(chain_changed(recovery.range_start())));
            recovery.target_hash=rpc.block_hash(recovery.target).await?;
            recovery.phase=Phase::Downloading;
            write_json(path,&recovery)?;
        }
        if matches!(recovery.phase,Phase::Complete|Phase::Reconciling) { return Ok(()); }
        if info["pruned"].as_bool()!=Some(true) || info["automatic_pruning"].as_bool()!=Some(false) {
            return Err("Restart the managed node with recovery pruning controls before downloading blocks".into());
        }
        let wallet = rpc.call(true,"getwalletinfo",json!([])).await.map_err(|e|e.to_string())?;
        if wallet["scanning"].is_object() { return Ok(()); }
        if wallet["scanning"].as_bool()!=Some(false) { return Err("Cannot determine local wallet scan status".into()); }
        if !recovery.blocks.is_empty() {
            return single_blocks_step(rpc, directory, path, &mut recovery).await;
        }
        let end=recovery.next.saturating_add(BATCH_BLOCKS-1).min(recovery.target);
        recovery.phase=Phase::Downloading;
        write_json(path,&recovery)?;
        let mut hashes=Vec::new();
        for height in recovery.next..=end {
            if fs4::available_space(directory).map_err(|e|e.to_string())? < MIN_FREE_BYTES { return Err("Not enough free disk space to recover more blocks; free space and resume".into()); }
            let hash=rpc.block_hash(height).await?;
            if !rpc.fetch_block(hash,recovery.fetch_attempts).await? {
                recovery.fetch_attempts+=1;
                if recovery.fetch_attempts>=8 { return Err("An archival peer did not supply the requested block; retry recovery after checking node connectivity".into()); }
                return Ok(());
            }
            hashes.push((height,hash));
        }
        recovery.phase=Phase::Scanning;
        write_json(path,&recovery)?;
        // The final scan omits stop_height to also reconcile transactions that
        // were already in the mempool before their funding parents were imported.
        let params=if end==recovery.target { json!([recovery.next]) } else {json!([recovery.next,end])};
        let scanned=rpc.call(true,"rescanblockchain",params).await.map_err(|e|e.to_string())?;
        if scanned["start_height"].as_u64()!=Some(u64::from(recovery.next)) || scanned["stop_height"].as_u64().is_none_or(|h|h<u64::from(end)) {
            return Err("The wallet did not finish scanning the requested blocks; resume recovery".into());
        }
        for (height,hash) in hashes { if rpc.block_hash(height).await? != hash { return Err("The chain changed during recovery; resume to scan the new branch".into()); } }
        recovery.next=end+1;
        recovery.fetch_attempts=0;
        recovery.phase=if end==recovery.target {Phase::Reconciling} else {Phase::Downloading};
        Ok(())
    }.await;
    if let Err(error) = result {
        let paused = absorb_transient(&mut recovery, &error);
        write_json(path, &recovery)?;
        return if paused { Err(error) } else { Ok(recovery) };
    }
    // A step that only found a scan still running, or a block not yet
    // delivered, made no progress: it must not reset the failure count, or a
    // scan that always outlives its RPC deadline would retry forever.
    let advanced = (recovery.next, recovery.blocks.len()) != before
        || matches!(recovery.phase, Phase::Reconciling | Phase::Complete);
    if advanced || recovery.transient_failures == 0 {
        recovery.last_error = None;
        recovery.transient_failures = 0;
    }
    write_json(path, &recovery)?;
    Ok(recovery)
}

/// Fetch and scan queued single blocks, one at a time, in height order so a
/// funding transaction is in the wallet before the block that spends it. Each
/// scanned block is persisted before the next, so an interruption repeats at
/// most one block.
async fn single_blocks_step(
    rpc: &NodeRpc,
    directory: &Path,
    path: &Path,
    recovery: &mut Recovery,
) -> Result<(), String> {
    let tail = recovery.tail.ok_or("Recovery block queue has no tail")?;
    for _ in 0..BATCH_BLOCKS {
        let Some(&height) = recovery.blocks.first() else {
            break;
        };
        if fs4::available_space(directory).map_err(|e| e.to_string())? < MIN_FREE_BYTES {
            return Err(
                "Not enough free disk space to recover more blocks; free space and resume".into(),
            );
        }
        recovery.phase = Phase::Downloading;
        write_json(path, recovery)?;
        let hash = rpc.block_hash(height).await?;
        if !rpc.fetch_block(hash, recovery.fetch_attempts).await? {
            recovery.fetch_attempts += 1;
            if recovery.fetch_attempts >= 8 {
                return Err("An archival peer did not supply the requested block; retry recovery after checking node connectivity".into());
            }
            return Ok(());
        }
        recovery.phase = Phase::Scanning;
        write_json(path, recovery)?;
        let scanned = rpc
            .call(true, "rescanblockchain", json!([height, height]))
            .await
            .map_err(|e| e.to_string())?;
        if scanned["start_height"].as_u64() != Some(u64::from(height))
            || scanned["stop_height"].as_u64() != Some(u64::from(height))
        {
            return Err(
                "The wallet did not finish scanning the requested block; resume recovery".into(),
            );
        }
        if rpc.block_hash(height).await? != hash {
            return Err("The chain changed during recovery; resume to scan the new branch".into());
        }
        recovery.blocks.remove(0);
        recovery.next = recovery.blocks.first().copied().unwrap_or(tail);
        recovery.fetch_attempts = 0;
        recovery.phase = Phase::Downloading;
        write_json(path, recovery)?;
    }
    Ok(())
}

/// Import known confirmed transactions in chain order. Unsupported imports
/// (including pure outgoing transactions) return block heights for scanning.
/// Missing proofs likewise fall back to scanning, never dropping old records.
pub async fn import_transactions(
    rpc: &NodeRpc,
    source: &EsploraConfig,
    transactions: &[coincubed::commands::TransactionInfo],
) -> Result<Vec<u32>, String> {
    let mut ordered: Vec<_> = transactions
        .iter()
        .filter_map(|tx| {
            tx.height
                .and_then(|h| u32::try_from(h).ok())
                .map(|height| (height, tx))
        })
        .collect();
    ordered.sort_by_key(|(height, _)| *height);
    let mut recover = BTreeMap::new();
    for (height, tx) in ordered {
        let txid = tx.tx.compute_txid();
        if rpc.transaction_at(txid, Some(height)).await?.is_some() {
            continue;
        }
        let block = rpc.block_hash(height).await?;
        let proof = match fetch_proof(source, txid, block).await {
            Ok(p) => p,
            Err(_) => {
                recover.insert(height, ());
                continue;
            }
        };
        let proof = verify_proof(&proof, txid, block)?;
        match rpc
            .call(
                true,
                "importprunedfunds",
                json!([serialize_hex(&tx.tx), serialize_hex(&proof)]),
            )
            .await
        {
            Ok(_) if rpc.transaction_at(txid, Some(height)).await?.is_some() => {}
            Ok(_) => return Err("Imported transaction is missing from the local wallet".into()),
            Err(e) if e.code == Some(-5) => {
                recover.insert(height, ());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(recover.into_keys().collect())
}

/// Proofs do not establish unspentness. A vanished output requires a scan to
/// find its spender, unless the spender (known to Connect's wallet view, or
/// named by `outspends`) is already recorded locally.
/// Returns the earliest height that still needs scanning.
pub async fn reconcile_coins(
    rpc: &NodeRpc,
    coins: &[coincubed::commands::ListCoinsEntry],
    outspends: Option<&EsploraConfig>,
) -> Result<Option<u32>, String> {
    Ok(plan_coin_scans(rpc, coins, outspends).await?.earliest())
}

/// Work out which blocks to scan so the local wallet holds every coin and
/// spend Connect reports. A coin points at the block of its missing funding
/// or spending transaction. With `outspends`, a spender Connect's wallet view
/// lacks is looked up there; a spender that stays unknown, or a pending one,
/// needs every block from the coin's own height.
pub async fn plan_coin_scans(
    rpc: &NodeRpc,
    coins: &[coincubed::commands::ListCoinsEntry],
    outspends: Option<&EsploraConfig>,
) -> Result<ScanPlan, String> {
    let mut plan = ScanPlan::default();
    for coin in coins {
        let height = coin
            .block_height
            .map(|height| u32::try_from(height).map_err(|_| "Invalid confirmed coin height"))
            .transpose()?;
        if height.is_none() {
            let funding = rpc.transaction_at(coin.outpoint.txid, None).await?;
            if funding.is_none() {
                return Err("Connect has an unconfirmed transaction the local node has not seen. Keep Connect active until the nodes agree, then resume recovery.".into());
            }
            if let Some(spend) = coin.spend_info {
                if rpc
                    .transaction_at(spend.txid, spend.height.and_then(|h| u32::try_from(h).ok()))
                    .await?
                    .is_none_or(|tx| {
                        !tx.input
                            .iter()
                            .any(|input| input.previous_output == coin.outpoint)
                    })
                {
                    return Err("The local node has not seen this pending spend. Keep Connect active until the nodes agree, then resume recovery.".into());
                }
            }
            let unspent = rpc
                .call(
                    false,
                    "gettxout",
                    json!([coin.outpoint.txid.to_string(), coin.outpoint.vout, true]),
                )
                .await
                .map_err(|e| e.to_string())?;
            if unspent.is_null() != coin.spend_info.is_some() {
                return Err("Connect and the local node disagree about a pending output's spender. Keep Connect active and resume when they agree.".into());
            }
            continue;
        }
        let height = height.expect("unconfirmed handled above");
        let funding = rpc.transaction_at(coin.outpoint.txid, Some(height)).await?;
        if funding.is_none() {
            plan.blocks.insert(height);
        }
        let OutPoint { txid, vout } = coin.outpoint;
        let unspent = rpc
            .call(false, "gettxout", json!([txid.to_string(), vout, true]))
            .await
            .map_err(|e| e.to_string())?;
        if let Some(spend) = coin.spend_info {
            let spend_height = spend.height.and_then(|h| u32::try_from(h).ok());
            let spender = rpc.transaction_at(spend.txid, spend_height).await?;
            let missing = spender.is_none_or(|tx| {
                !tx.input
                    .iter()
                    .any(|input| input.previous_output == coin.outpoint)
            }) || !unspent.is_null();
            if missing {
                match spend_height {
                    Some(spent_at) => {
                        plan.blocks.insert(spent_at);
                    }
                    None => plan.lower_floor(
                        height,
                        format!("A pending spend of a coin from block {height} is not in the local wallet yet, so COINCUBE Desktop scans every block from {height}."),
                    ),
                }
            }
        } else if unspent.is_null() {
            // `Some(None)`: the local wallet already holds the named spender.
            let found = match outspends {
                Some(source) => match fetch_outspend(source, coin.outpoint).await {
                    // The provider's height can be past the local tip or wrong;
                    // either way the coin falls back to a full scan. A node that
                    // missed its deadline is retried instead.
                    Ok(Some((spender, spent_at, block))) if spent_at >= height => {
                        match rpc.block_hash(spent_at).await {
                            Ok(local) if local == block => {
                                let recorded = rpc
                                    .transaction_at(spender, Some(spent_at))
                                    .await?
                                    .is_some_and(|tx| {
                                        tx.input
                                            .iter()
                                            .any(|input| input.previous_output == coin.outpoint)
                                    });
                                Some((!recorded).then_some(spent_at))
                            }
                            Ok(_) => None,
                            Err(e) if e.starts_with(UNREACHABLE) => return Err(e),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                },
                None => None,
            };
            match found {
                Some(Some(spent_at)) => {
                    plan.blocks.insert(spent_at);
                }
                Some(None) => {}
                None => plan.lower_floor(
                    height,
                    format!("Connect could not say which transaction spent a coin from block {height}, so COINCUBE Desktop scans every block from {height}."),
                ),
            }
        } else {
            let output = funding.as_ref().and_then(|tx| tx.output.get(vout as usize));
            let script = unspent["scriptPubKey"]["hex"]
                .as_str()
                .ok_or("Invalid unspent output script")?;
            let amount = unspent["value"]
                .as_f64()
                .and_then(|value| bitcoin::Amount::from_btc(value).ok())
                .ok_or("Invalid unspent output amount")?;
            if output.is_none_or(|output| {
                output.value != amount
                    || output.script_pubkey.to_hex_string() != script
                    || amount != coin.amount
            }) {
                plan.blocks.insert(height);
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn genesis() -> bitcoin::Block {
        bitcoin::blockdata::constants::genesis_block(Network::Regtest)
    }
    fn binding() -> Binding {
        Binding {
            genesis: genesis().block_hash(),
            node_instance: "test-node".into(),
            endpoint: "fixture".into(),
            wallet: "history-fixture".into(),
            descriptor: "fixture".into(),
        }
    }
    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("history-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    async fn rpc(server: &MockServer) -> NodeRpc {
        NodeRpc::new(
            &BitcoindConfig {
                addr: *server.address(),
                rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                    "fixture".into(),
                    "fixture".into(),
                ),
            },
            "history-fixture",
            crate::chain::ChainId::from(Network::Regtest),
        )
        .await
        .unwrap()
    }
    async fn response(server: &MockServer, method: &str, result: Value) {
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(json!({"method":method}).to_string());
                then.status(200)
                    .json_body(json!({"id":"history","result":result}));
            })
            .await;
    }
    async fn base(server: &MockServer, automatic: bool) {
        response(
            server,
            "getblockhash",
            json!(genesis().block_hash().to_string()),
        )
        .await;
        response(server,"getblockchaininfo",json!({"pruned":true,"automatic_pruning":automatic,"initialblockdownload":false,"blocks":0,"pruneheight":999})).await;
        response(server, "getwalletinfo", json!({"scanning":false})).await;
        response(server, "getblock", json!(serialize_hex(&genesis()))).await;
    }

    #[test]
    fn inclusion_proof_requires_transaction_and_active_chain_block() {
        let block = genesis();
        let txid = block.txdata[0].compute_txid();
        let proof = MerkleBlock::from_block_with_predicate(&block, |t| *t == txid);
        let bytes = bitcoin::consensus::serialize(&proof);
        assert!(verify_proof(&bytes, txid, block.block_hash()).is_ok());
        assert!(verify_proof(&bytes, Txid::from_byte_array([42; 32]), block.block_hash()).is_err());
        assert!(verify_proof(&bytes, txid, BlockHash::from_byte_array([42; 32])).is_err());
        assert!(verify_proof(&[0; 8], txid, block.block_hash()).is_err());
        let mut bad = proof;
        bad.header.merkle_root = bitcoin::TxMerkleNode::from_byte_array([42; 32]);
        assert!(verify_proof(
            &bitcoin::consensus::serialize(&bad),
            txid,
            bad.header.block_hash()
        )
        .is_err());
    }

    #[test]
    fn checkpoint_is_scoped_and_bounds_checked() {
        let mut recovery = Recovery::new(binding(), 100, 200, genesis().block_hash()).unwrap();
        assert!(recovery.validate(&binding()).is_ok());
        for field in ["wallet", "descriptor", "endpoint", "node_instance"] {
            let mut other = binding();
            match field {
                "wallet" => other.wallet.push('x'),
                "descriptor" => other.descriptor.push('x'),
                "endpoint" => other.endpoint.push('x'),
                _ => other.node_instance.push('x'),
            };
            assert!(recovery.validate(&other).is_err());
        }
        let mut other = binding();
        other.genesis = BlockHash::from_byte_array([42; 32]);
        assert!(recovery.validate(&other).is_err());
        recovery.next = 99;
        assert!(recovery.validate(&binding()).is_err());
        recovery.next = 202;
        assert!(recovery.validate(&binding()).is_err());
        assert!(Recovery::new(binding(), 201, 200, genesis().block_hash()).is_err());
    }

    #[test]
    fn progress_storage_is_atomic_and_controller_excludes_other_wallets() {
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        let mut recovery = Recovery::new(binding(), 0, 100, genesis().block_hash()).unwrap();
        write_json(&path, &recovery).unwrap();
        let first = HistoryLease::acquire(&dir).unwrap();
        assert!(HistoryLease::acquire(&dir).is_err());
        recovery.next = 32;
        write_json(&path, &recovery).unwrap();
        let read: Recovery = read_json(&path).unwrap().unwrap();
        assert_eq!(read.next, 32);
        assert!(read_json::<Recovery>(&dir.join("missing"))
            .unwrap()
            .is_none());
        drop(first);
        assert!(HistoryLease::acquire(&dir).is_ok());
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read_json::<Recovery>(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn slow_wallet_load_outlives_the_normal_rpc_timeout() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(200)
                    .delay(Duration::from_secs(11))
                    .json_body(json!({"id":"history", "result":{}}));
            })
            .await;
        let client = rpc(&server).await;
        let (loaded, ordinary) = tokio::join!(
            client.call(false, "loadwallet", json!(["fixture"])),
            client.call(false, "getwalletinfo", json!([])),
        );
        assert!(loaded.is_ok());
        assert!(ordinary.is_err());
    }

    #[tokio::test]
    async fn scheduled_block_download_is_polled_without_rescheduling() {
        let server = MockServer::start_async().await;
        let missing = server.mock_async(|when, then| {
            when.method(POST).json_body_partial(r#"{"method":"getblock"}"#);
            then.status(200).json_body(json!({"id":"history", "error":{"code":-1,"message":"Block not available (pruned data)"}}));
        }).await;
        response(
            &server,
            "getpeerinfo",
            json!([{"id":7,"inbound":false,"services":"0000000000000009"}]),
        )
        .await;
        let scheduled = server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(r#"{"method":"getblockfrompeer"}"#);
                then.status(200)
                    .json_body(json!({"id":"history", "result":{}}));
            })
            .await;
        let client = rpc(&server).await;
        let arriving = async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            missing.delete_async().await;
            response(&server, "getblock", json!(serialize_hex(&genesis()))).await;
        };
        let (downloaded, ()) =
            tokio::join!(client.fetch_block(genesis().block_hash(), 0), arriving);
        assert!(downloaded.unwrap());
        scheduled.assert_async().await;
    }

    #[tokio::test]
    async fn recovery_reads_block_bodies_despite_unchanged_pruneheight() {
        let server = MockServer::start_async().await;
        base(&server, false).await;
        response(
            &server,
            "rescanblockchain",
            json!({"start_height":0,"stop_height":0}),
        )
        .await;
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        write_json(
            &path,
            &Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap(),
        )
        .unwrap();
        let result = recover_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(result.phase, Phase::Reconciling);
        assert_eq!(result.next, 1);
        assert_eq!(result.progress(), 1.0);
        let saved: Recovery = read_json(&path).unwrap().unwrap();
        assert_eq!(saved.phase, Phase::Reconciling);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn automatic_pruning_and_short_or_aborted_scans_never_advance_progress() {
        for automatic in [true, false] {
            for stop in [Value::Null, json!(-1)] {
                let server = MockServer::start_async().await;
                base(&server, automatic).await;
                let scan = server
                    .mock_async(|when, then| {
                        when.method(POST)
                            .json_body_partial(r#"{"method":"rescanblockchain"}"#);
                        then.status(200).json_body(
                            json!({"id":"history","result":{"start_height":0,"stop_height":stop}}),
                        );
                    })
                    .await;
                let dir = directory();
                let path = recovery_path(&dir, "fixture");
                write_json(
                    &path,
                    &Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap(),
                )
                .unwrap();
                assert!(
                    recover_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
                        .await
                        .is_err()
                );
                let saved: Recovery = read_json(&path).unwrap().unwrap();
                assert_eq!(saved.next, 0);
                assert_eq!(saved.phase, Phase::Paused);
                scan.assert_hits_async(if automatic { 0 } else { 1 }).await;
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn funding_inclusion_does_not_establish_unspentness_and_stale_spends_are_rejected() {
        let funding = genesis().txdata[0].clone();
        let txid = funding.compute_txid();
        let mut spender = funding.clone();
        spender.input[0].previous_output = OutPoint { txid, vout: 0 };
        let spendid = spender.compute_txid();
        for stale_spend in [false, true] {
            let server = MockServer::start_async().await;
            response(
                &server,
                "getblockhash",
                json!(genesis().block_hash().to_string()),
            )
            .await;
            response(&server, "gettxout", Value::Null).await;
            for (id, tx, confirmations) in [(txid, &funding, 1), (spendid, &spender, 0)] {
                server.mock_async(|when,then| {
                    when.method(POST).json_body_partial(json!({"method":"gettransaction","params":[id.to_string(),true]}).to_string());
                    then.status(200).json_body(json!({"id":"history","result":{"txid":id.to_string(),"hex":serialize_hex(tx),"confirmations":confirmations,"blockhash":genesis().block_hash().to_string()}}));
                }).await;
            }
            server.mock_async(|when,then| {
                when.method(POST).json_body_partial(r#"{"method":"getmempoolentry"}"#);
                then.status(500).json_body(json!({"id":"history","error":{"code":-5,"message":"Transaction not in mempool"}}));
            }).await;
            let coin = coincubed::commands::ListCoinsEntry {
                amount: funding.output[0].value,
                outpoint: OutPoint { txid, vout: 0 },
                address: bitcoin::Address::p2wsh(&bitcoin::ScriptBuf::new(), Network::Regtest),
                block_height: Some(0),
                derivation_index: bitcoin::bip32::ChildNumber::from_normal_idx(0).unwrap(),
                spend_info: if stale_spend {
                    Some(coincubed::commands::LCSpendInfo {
                        txid: spendid,
                        height: None,
                    })
                } else {
                    None
                },
                is_immature: false,
                is_change: false,
                is_from_self: false,
            };
            assert_eq!(
                reconcile_coins(&rpc(&server).await, &[coin], None)
                    .await
                    .unwrap(),
                Some(0)
            );
        }
    }

    #[tokio::test]
    async fn bootstrap_downloads_without_publishing_a_scan_checkpoint() {
        let server = MockServer::start_async().await;
        base(&server, false).await;
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        let mut job = Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap();
        job.phase = Phase::FetchingForLoad;
        write_json(&path, &job).unwrap();
        let job = bootstrap_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(job.next, 0);
        assert_eq!(job.download_next, 1);
        assert_eq!(job.phase, Phase::FetchingForLoad);
        assert!(!job.wallet_ready);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn chain_reorganization_rewinds_a_completed_scan_before_reconciliation() {
        let server = MockServer::start_async().await;
        base(&server, false).await;
        response(
            &server,
            "rescanblockchain",
            json!({"start_height":0,"stop_height":0}),
        )
        .await;
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        let mut job = Recovery::new(binding(), 0, 0, BlockHash::from_byte_array([42; 32])).unwrap();
        job.next = 1;
        job.phase = Phase::Complete;
        job.db_replayed = true;
        write_json(&path, &job).unwrap();
        let job = recover_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(job.target_hash, genesis().block_hash());
        assert_eq!(job.phase, Phase::Reconciling);
        assert_eq!(job.next, 1);
        assert!(!job.db_replayed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn invalid_primary_proof_falls_back_without_disclosing_its_token() {
        let primary = MockServer::start_async().await;
        let fallback = MockServer::start_async().await;
        let block = genesis();
        let txid = block.txdata[0].compute_txid();
        let path = format!("/tx/{txid}/merkleblock-proof");
        let rejected = primary
            .mock_async(|when, then| {
                when.method(GET)
                    .path(&path)
                    .header("authorization", "Bearer primary-fixture");
                then.status(200).body("0000");
            })
            .await;
        let proof = MerkleBlock::from_block_with_predicate(&block, |t| *t == txid);
        let accepted = fallback
            .mock_async(|when, then| {
                when.method(GET)
                    .path(&path)
                    .header("authorization", "Bearer fallback-fixture");
                then.status(200).body(serialize_hex(&proof));
            })
            .await;
        let config: EsploraConfig = serde_json::from_value(json!({
            "addr":primary.base_url(), "token":"primary-fixture",
            "fallback_addr":fallback.base_url(), "fallback_token":"fallback-fixture"
        }))
        .unwrap();
        let bytes = fetch_proof(&config, txid, block.block_hash())
            .await
            .unwrap();
        assert!(verify_proof(&bytes, txid, block.block_hash()).is_ok());
        rejected.assert_async().await;
        accepted.assert_async().await;
    }

    #[tokio::test]
    async fn failed_scan_response_preserves_a_resumable_checkpoint() {
        let server = MockServer::start_async().await;
        base(&server, false).await;
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(r#"{"method":"rescanblockchain"}"#);
                then.status(503).body("connection interrupted");
            })
            .await;
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        write_json(
            &path,
            &Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap(),
        )
        .unwrap();
        assert!(
            recover_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
                .await
                .is_err()
        );
        let saved: Recovery = read_json(&path).unwrap().unwrap();
        assert_eq!(saved.next, 0);
        assert_eq!(saved.phase, Phase::Paused);
        assert!(saved.last_error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rpc_failures_do_not_become_missing_transactions() {
        let server = MockServer::start_async().await;
        let failed = server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(500)
                    .json_body(json!({"id":"history","error":{"code":-28,"message":"Loading"}}));
            })
            .await;
        let client = rpc(&server).await;
        assert!(client
            .knows_transaction(genesis().txdata[0].compute_txid())
            .await
            .is_err());
        failed.assert_async().await;
    }

    #[tokio::test]
    async fn unknown_transaction_is_absence_but_mismatched_response_is_not() {
        for id in ["history", "other-request"] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST);
                    then.status(200).json_body(
                        json!({"id":id,"error":{"code":-5,"message":"Unknown transaction"}}),
                    );
                })
                .await;
            let result = rpc(&server)
                .await
                .knows_transaction(genesis().txdata[0].compute_txid())
                .await;
            if id == "history" {
                assert!(!result.unwrap());
            } else {
                assert!(result.is_err());
            }
        }
    }

    fn coin(spend: Option<(Txid, Option<i32>)>) -> coincubed::commands::ListCoinsEntry {
        let funding = genesis().txdata[0].clone();
        coincubed::commands::ListCoinsEntry {
            amount: funding.output[0].value,
            outpoint: OutPoint {
                txid: funding.compute_txid(),
                vout: 0,
            },
            address: bitcoin::Address::p2wsh(&bitcoin::ScriptBuf::new(), Network::Regtest),
            block_height: Some(0),
            derivation_index: bitcoin::bip32::ChildNumber::from_normal_idx(0).unwrap(),
            spend_info: spend
                .map(|(txid, height)| coincubed::commands::LCSpendInfo { txid, height }),
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    /// The funding transaction is in the wallet at height 0 and the node
    /// reports its output spent; any other wallet lookup is unknown.
    async fn spent_coin_node(server: &MockServer) {
        let funding = genesis().txdata[0].clone();
        let txid = funding.compute_txid();
        response(
            server,
            "getblockhash",
            json!(genesis().block_hash().to_string()),
        )
        .await;
        response(server, "gettxout", Value::Null).await;
        server.mock_async(|when,then| {
            when.method(POST).json_body_partial(json!({"method":"gettransaction","params":[txid.to_string(),true]}).to_string());
            then.status(200).json_body(json!({"id":"history","result":{"txid":txid.to_string(),"hex":serialize_hex(&funding),"confirmations":1,"blockhash":genesis().block_hash().to_string()}}));
        }).await;
        server.mock_async(|when,then| {
            when.method(POST).json_body_partial(r#"{"method":"gettransaction"}"#);
            then.status(500).json_body(json!({"id":"history","error":{"code":-5,"message":"Invalid or non-wallet transaction id"}}));
        }).await;
    }

    #[test]
    fn planned_blocks_keep_next_at_the_lowest_unscanned_height() {
        let mut job = Recovery::new(binding(), 0, 100, genesis().block_hash()).unwrap();
        job.plan(BTreeSet::from([5, 40, 70, 90]), 69, None);
        // 70 and 90 sit inside the tail, which scans them anyway.
        assert_eq!(job.blocks, vec![5, 40]);
        assert_eq!((job.start, job.next, job.tail), (5, 5, Some(69)));
        assert_eq!(job.full_start, Some(0));
        job.full_start = Some(6);
        assert!(job.validate(&binding()).is_err(), "full start above start");
        job.full_start = Some(0);
        assert!(job.validate(&binding()).is_ok());
        assert_eq!(job.block_counts(), (0, 2 + 32));
        job.blocks.remove(0);
        job.next = 40;
        assert_eq!(job.block_counts(), (1, 34));
        // `next` must name the first queued block.
        job.next = 41;
        assert!(job.validate(&binding()).is_err());
        job.next = 40;
        job.blocks.clear();
        assert!(job.validate(&binding()).is_err(), "tail not reached yet");
        job.next = 69;
        assert!(job.validate(&binding()).is_ok());
        assert_eq!(job.block_counts(), (2, 34));
        // A rewind scans the whole requested range, not just from the first
        // queued block.
        job.rewind(Some("why".into()));
        assert_eq!(
            (job.start, job.next, job.tail, job.planned),
            (0, 0, None, 0)
        );
        assert_eq!(job.full_start, None);
        assert_eq!(job.fallback.as_deref(), Some("why"));
        assert_eq!(job.block_counts(), (0, 101));
        assert!(job.validate(&binding()).is_ok());
        // A plan with nothing to fetch scans only its tail.
        let mut job = Recovery::new(binding(), 0, 100, genesis().block_hash()).unwrap();
        job.plan(BTreeSet::new(), 69, None);
        assert_eq!((job.start, job.next), (69, 69));
        assert!(job.validate(&binding()).is_ok());
        // Coins newer than the tail: the plan starts lower than the request,
        // and the journal must stay valid and rewind from the lower start.
        let mut job = Recovery::new(binding(), 90, 100, genesis().block_hash()).unwrap();
        job.plan(BTreeSet::new(), 69, None);
        assert_eq!((job.start, job.full_start), (69, Some(69)));
        assert!(job.validate(&binding()).is_ok());
        job.rewind(None);
        assert_eq!((job.start, job.next), (69, 69));
        assert!(job.validate(&binding()).is_ok());
    }

    #[test]
    fn a_missed_node_deadline_keeps_running_until_the_retry_budget_ends() {
        let mut job = Recovery::new(binding(), 0, 100, genesis().block_hash()).unwrap();
        let unreachable = format!("{UNREACHABLE}getblockhash");
        for attempt in 1..=MAX_TRANSIENT_FAILURES {
            assert!(!absorb_transient(&mut job, &unreachable));
            assert_eq!(job.phase, Phase::Downloading);
            assert_eq!(job.transient_failures, attempt);
        }
        assert!(absorb_transient(&mut job, &unreachable));
        assert_eq!(job.phase, Phase::Paused);
        assert_eq!(job.transient_failures, 0);
        let mut job = Recovery::new(binding(), 0, 100, genesis().block_hash()).unwrap();
        assert!(absorb_transient(&mut job, "Not enough free disk space"));
        assert_eq!(job.phase, Phase::Paused);
    }

    #[tokio::test]
    async fn an_unreachable_node_does_not_pause_recovery() {
        // httpmock pools its servers, so a dropped one can still answer; a
        // closed listener's port refuses the connection.
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let rpc = NodeRpc::new(
            &BitcoindConfig {
                addr: address,
                rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                    "fixture".into(),
                    "fixture".into(),
                ),
            },
            "history-fixture",
            crate::chain::ChainId::from(Network::Regtest),
        )
        .await
        .unwrap();
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        write_json(
            &path,
            &Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap(),
        )
        .unwrap();
        let job = recover_step(&rpc, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(job.phase, Phase::Downloading);
        assert_eq!(job.transient_failures, 1);
        assert!(job.last_error.unwrap().contains("Retrying"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_scan_still_running_does_not_reset_the_failure_count() {
        let server = MockServer::start_async().await;
        response(
            &server,
            "getblockhash",
            json!(genesis().block_hash().to_string()),
        )
        .await;
        response(&server,"getblockchaininfo",json!({"pruned":true,"automatic_pruning":false,"initialblockdownload":false,"blocks":0,"pruneheight":999})).await;
        response(
            &server,
            "getwalletinfo",
            json!({"scanning":{"duration":130,"progress":0.5}}),
        )
        .await;
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        let mut job = Recovery::new(binding(), 0, 0, genesis().block_hash()).unwrap();
        job.phase = Phase::Scanning;
        job.transient_failures = 3;
        job.last_error = Some("Cannot reach local node for rescanblockchain. Retrying…".into());
        write_json(&path, &job).unwrap();
        let job = recover_step(&rpc(&server).await, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(job.transient_failures, 3);
        assert!(job.last_error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_outspend_past_the_local_tip_falls_back_instead_of_failing() {
        let funding = genesis().txdata[0].compute_txid();
        let server = MockServer::start_async().await;
        // Registered first, so it wins over the generic getblockhash below.
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(r#"{"method":"getblockhash"}"#)
                    .body_contains("\"params\":[7]");
                then.status(500).json_body(json!({"id":"history","error":{"code":-8,"message":"Block height out of range"}}));
            })
            .await;
        spent_coin_node(&server).await;
        let esplora = MockServer::start_async().await;
        esplora
            .mock_async(|when, then| {
                when.method(GET).path(format!("/tx/{funding}/outspend/0"));
                then.status(200).json_body(json!({"spent":true,"txid":Txid::from_byte_array([7;32]).to_string(),"vin":0,"status":{"confirmed":true,"block_height":7,"block_hash":genesis().block_hash().to_string()}}));
            })
            .await;
        let config: EsploraConfig =
            serde_json::from_value(json!({"addr":esplora.base_url()})).unwrap();
        let plan = plan_coin_scans(&rpc(&server).await, &[coin(None)], Some(&config))
            .await
            .unwrap();
        assert!(plan.blocks.is_empty());
        assert_eq!(plan.floor.map(|(height, _)| height), Some(0));
    }

    #[tokio::test]
    async fn an_unreachable_node_while_checking_an_outspend_is_retried_not_scanned() {
        let funding = genesis().txdata[0].compute_txid();
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .json_body_partial(r#"{"method":"getblockhash"}"#)
                    .body_contains("\"params\":[7]");
                then.status(200)
                    .delay(Duration::from_secs(11))
                    .json_body(json!({"id":"history","result":genesis().block_hash().to_string()}));
            })
            .await;
        spent_coin_node(&server).await;
        let esplora = MockServer::start_async().await;
        esplora
            .mock_async(|when, then| {
                when.method(GET).path(format!("/tx/{funding}/outspend/0"));
                then.status(200).json_body(json!({"spent":true,"txid":Txid::from_byte_array([7;32]).to_string(),"vin":0,"status":{"confirmed":true,"block_height":7,"block_hash":genesis().block_hash().to_string()}}));
            })
            .await;
        let config: EsploraConfig =
            serde_json::from_value(json!({"addr":esplora.base_url()})).unwrap();
        let error = plan_coin_scans(&rpc(&server).await, &[coin(None)], Some(&config))
            .await
            .unwrap_err();
        assert!(error.starts_with(UNREACHABLE), "{}", error);
    }

    #[tokio::test]
    async fn a_scanned_spender_connect_does_not_know_reconciles() {
        let funding = genesis().txdata[0].compute_txid();
        let spender = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: OutPoint {
                    txid: funding,
                    vout: 0,
                },
                ..Default::default()
            }],
            output: vec![],
        };
        let spendid = spender.compute_txid();
        let esplora = MockServer::start_async().await;
        esplora
            .mock_async(|when, then| {
                when.method(GET).path(format!("/tx/{funding}/outspend/0"));
                then.status(200).json_body(json!({"spent":true,"txid":spendid.to_string(),"vin":0,"status":{"confirmed":true,"block_height":7,"block_hash":genesis().block_hash().to_string()}}));
            })
            .await;
        let config: EsploraConfig =
            serde_json::from_value(json!({"addr":esplora.base_url()})).unwrap();
        for scanned in [false, true] {
            let server = MockServer::start_async().await;
            if scanned {
                // Registered first, so it wins over the unknown-transaction mock.
                server.mock_async(|when,then| {
                    when.method(POST).json_body_partial(json!({"method":"gettransaction","params":[spendid.to_string(),true]}).to_string());
                    then.status(200).json_body(json!({"id":"history","result":{"txid":spendid.to_string(),"hex":serialize_hex(&spender),"confirmations":1,"blockhash":genesis().block_hash().to_string()}}));
                }).await;
            }
            spent_coin_node(&server).await;
            let rpc = rpc(&server).await;
            // Before the scan the spend's block is queued; after it, Connect
            // still has no spender, but the local wallet proves the spend.
            assert_eq!(
                reconcile_coins(&rpc, &[coin(None)], Some(&config))
                    .await
                    .unwrap(),
                (!scanned).then_some(7)
            );
            assert_eq!(
                reconcile_coins(&rpc, &[coin(None)], None).await.unwrap(),
                Some(0)
            );
        }
        // A held transaction under the named txid that spends some other
        // output proves nothing; its block stays queued.
        let mut other = spender.clone();
        other.input[0].previous_output.vout = 1;
        let otherid = other.compute_txid();
        let esplora = MockServer::start_async().await;
        esplora
            .mock_async(|when, then| {
                when.method(GET).path(format!("/tx/{funding}/outspend/0"));
                then.status(200).json_body(json!({"spent":true,"txid":otherid.to_string(),"vin":0,"status":{"confirmed":true,"block_height":7,"block_hash":genesis().block_hash().to_string()}}));
            })
            .await;
        let config: EsploraConfig =
            serde_json::from_value(json!({"addr":esplora.base_url()})).unwrap();
        let server = MockServer::start_async().await;
        server.mock_async(|when,then| {
            when.method(POST).json_body_partial(json!({"method":"gettransaction","params":[otherid.to_string(),true]}).to_string());
            then.status(200).json_body(json!({"id":"history","result":{"txid":otherid.to_string(),"hex":serialize_hex(&other),"confirmations":1,"blockhash":genesis().block_hash().to_string()}}));
        }).await;
        spent_coin_node(&server).await;
        assert_eq!(
            reconcile_coins(&rpc(&server).await, &[coin(None)], Some(&config))
                .await
                .unwrap(),
            Some(7)
        );
    }

    #[tokio::test]
    async fn queued_blocks_are_scanned_one_by_one_before_the_tail() {
        let server = MockServer::start_async().await;
        base(&server, false).await;
        let mut scans = vec![];
        for (params, start, stop) in [("[5,5]", 5, 5), ("[9,9]", 9, 9), ("[20]", 20, 20)] {
            scans.push(
                server
                    .mock_async(|when, then| {
                        when.method(POST)
                            .json_body_partial(r#"{"method":"rescanblockchain"}"#)
                            .body_contains(format!("\"params\":{params}"));
                        then.status(200).json_body(
                            json!({"id":"history","result":{"start_height":start,"stop_height":stop}}),
                        );
                    })
                    .await,
            );
        }
        let dir = directory();
        let path = recovery_path(&dir, "fixture");
        let mut job = Recovery::new(binding(), 0, 20, genesis().block_hash()).unwrap();
        job.plan(BTreeSet::from([5, 9]), 20, None);
        write_json(&path, &job).unwrap();
        let rpc = rpc(&server).await;
        let job = recover_step(&rpc, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert!(job.blocks.is_empty());
        assert_eq!((job.next, job.phase.clone()), (20, Phase::Downloading));
        assert_eq!(job.block_counts(), (2, 3));
        let job = recover_step(&rpc, Network::Regtest, &binding(), &path)
            .await
            .unwrap();
        assert_eq!(job.phase, Phase::Reconciling);
        assert_eq!(job.progress(), 1.0);
        for scan in scans {
            scan.assert_hits_async(1).await;
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_known_spend_needs_only_its_own_block() {
        let server = MockServer::start_async().await;
        spent_coin_node(&server).await;
        let spender = Txid::from_byte_array([7; 32]);
        let plan = plan_coin_scans(&rpc(&server).await, &[coin(Some((spender, Some(7))))], None)
            .await
            .unwrap();
        assert_eq!(plan.blocks, BTreeSet::from([7]));
        assert_eq!(plan.floor, None);
        // A pending spend cannot be fetched as a block.
        let plan = plan_coin_scans(&rpc(&server).await, &[coin(Some((spender, None)))], None)
            .await
            .unwrap();
        assert!(plan.blocks.is_empty());
        assert_eq!(plan.floor.map(|(height, _)| height), Some(0));
    }

    #[tokio::test]
    async fn an_unknown_spender_is_looked_up_before_scanning_every_block() {
        let funding = genesis().txdata[0].compute_txid();
        let path = format!("/tx/{funding}/outspend/0");
        for (status, body, expected) in [
            (
                200,
                json!({"spent":true,"txid":Txid::from_byte_array([7;32]).to_string(),"vin":0,"status":{"confirmed":true,"block_height":7,"block_hash":genesis().block_hash().to_string()}}),
                Some(7),
            ),
            (
                200,
                json!({"spent":true,"status":{"confirmed":true,"block_height":7,"block_hash":BlockHash::from_byte_array([42;32]).to_string()}}),
                None,
            ),
            (
                200,
                json!({"spent":true,"status":{"confirmed":false}}),
                None,
            ),
            (404, json!({}), None),
        ] {
            let server = MockServer::start_async().await;
            spent_coin_node(&server).await;
            let esplora = MockServer::start_async().await;
            esplora
                .mock_async(|when, then| {
                    when.method(GET).path(&path);
                    then.status(status).json_body(body.clone());
                })
                .await;
            let config: EsploraConfig =
                serde_json::from_value(json!({"addr":esplora.base_url()})).unwrap();
            let plan = plan_coin_scans(&rpc(&server).await, &[coin(None)], Some(&config))
                .await
                .unwrap();
            match expected {
                Some(height) => {
                    assert_eq!(plan.blocks, BTreeSet::from([height]));
                    assert_eq!(plan.floor, None);
                }
                None => {
                    assert!(plan.blocks.is_empty());
                    let (height, reason) = plan.floor.unwrap();
                    assert_eq!(height, 0);
                    assert!(reason.contains("every block from 0"));
                }
            }
        }
    }
}
