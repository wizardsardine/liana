//! Network activity behind the Launcher's Cubes: a rail of recent blocks with
//! the mempool queue after the tip, and up to five facts under it (block,
//! mempool, fee rate, hashrate, next halving).
//!
//! It follows the Network filter. Its source is a device preference:
//! Automatic reads the managed node when that node is already running and
//! otherwise COINCUBE | Connect's public Esplora proxy. The Launcher never
//! starts a node; a stopped node only means there is no cookie to read.
//! Nothing here moves at rest: a refresh is a message, and the one motion is
//! a short slide when a new block arrives, skipped under reduced motion.

use coincube_ui::locale::copy as localized_copy;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use iced::{
    widget::{container, text, Space},
    Alignment, Border, Color, Length, Padding, Subscription, Task,
};
use serde::Deserialize;
use serde_json::{json, Value};

use coincube_ui::{
    font, theme,
    widget::{Column, Container, Element, Row},
};

use super::{
    launcher_catalog::{CatalogMessage, NetworkFilter},
    launcher_scene, Home, Message, ViewMessage,
};
use crate::{
    app::settings::{global::GlobalSettings, preferences::NetworkActivitySource},
    chain::ChainId,
    dir::CoincubeDirectory,
};

/// How often a shown network is read again.
pub(super) const REFRESH: Duration = Duration::from_secs(60);
/// The check cadence: a first read lands within one tick of the Launcher
/// showing, and a filter or source change within one tick of the change.
const TICK: Duration = Duration::from_secs(5);
/// The new block's slide.
pub(super) const ARRIVAL: Duration = Duration::from_millis(900);
/// How far the rail slides in from the right, in pixels.
const ARRIVAL_SHIFT: f32 = 26.0;
/// Blocks on the rail, tip included. Esplora's `/blocks` returns ten.
const RAIL_BLOCKS: usize = 10;
/// One block's worth of virtual bytes (4M weight units).
const BLOCK_VSIZE: f64 = 1_000_000.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A mempool smaller than this reads as empty.
const EMPTY_BACKLOG: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    LocalNode,
    Connect,
    MempoolSpace,
}

impl Via {
    pub(super) fn label(self) -> &'static str {
        match self {
            Via::LocalNode => localized_copy("Local node"),
            Via::Connect => "COINCUBE | Connect",
            Via::MempoolSpace => "mempool.space",
        }
    }
}

/// The public explorer for `chain`: Connect for the networks its proxy
/// serves, mempool.space for the test networks it does not, and none for
/// regtest. Its base URL, and how the selector names it.
pub(super) fn public_source(chain: ChainId) -> Option<(Via, String)> {
    match chain {
        ChainId::Bitcoin | ChainId::BitcoinBlake2b | ChainId::BitcoinBlake2bTestnet4 => {
            Some((Via::Connect, connect_base(chain)))
        }
        ChainId::Testnet | ChainId::Testnet4 | ChainId::Signet => Some((
            Via::MempoolSpace,
            crate::installer::public_esplora_url(chain.bitcoin_network()),
        )),
        ChainId::Regtest => None,
    }
}

/// One read of a network. Blocks are oldest first, the tip last.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub tip: u32,
    pub tx_counts: Vec<u32>,
    pub mempool_vsize: u64,
    /// sat/vB to be included in the next block, when the source estimates it.
    pub next_block_fee: Option<f64>,
    pub difficulty: f64,
    pub via: Via,
}

/// Why nothing is shown. The dock's selector says it in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// The managed node is not set up for the network, not running, or its
    /// RPC failed: the read cannot tell these apart, so the label does not.
    LocalNodeNotRunning,
    /// The public explorer for the network did not answer.
    Unreachable(Via),
    /// Automatic found no local node and its public explorer did not answer.
    NeitherAnswered(Via),
    /// The network has no public explorer (regtest).
    NoSource,
}

impl Unavailable {
    pub(super) fn label(self) -> &'static str {
        match self {
            Unavailable::LocalNodeNotRunning => localized_copy("Local node didn't answer"),
            Unavailable::Unreachable(Via::MempoolSpace) => {
                localized_copy("mempool.space is unreachable")
            }
            Unavailable::Unreachable(_) => localized_copy("COINCUBE | Connect is unreachable"),
            Unavailable::NeitherAnswered(Via::MempoolSpace) => {
                localized_copy("Local node and mempool.space didn't answer")
            }
            Unavailable::NeitherAnswered(_) => {
                localized_copy("Local node and Connect didn't answer")
            }
            Unavailable::NoSource => localized_copy("No public explorer for this network"),
        }
    }
}

/// Why Automatic shows nothing once the local node has not answered: the
/// public explorer failed too, or the network has none and only a running
/// node could have served it. A failed local read is not proof the node is
/// stopped, so neither label says so.
fn automatic_unavailable(chain: ChainId) -> Unavailable {
    match public_source(chain) {
        Some((via, _)) => Unavailable::NeitherAnswered(via),
        None => Unavailable::LocalNodeNotRunning,
    }
}

#[derive(Debug, Clone)]
pub enum ActivityMessage {
    Tick(Instant),
    Loaded {
        request: u64,
        chain: ChainId,
        source: NetworkActivitySource,
        result: Result<Snapshot, Unavailable>,
    },
    Frame(Instant),
    /// Opens or closes the dock's source menu.
    Menu(bool),
    /// Follows Bitcoin Blake2b (`true`) or Bitcoin (`false`) where the
    /// Network filter names no single network.
    Blake2b(bool),
}

/// The Launcher's activity: its two device preferences and the last read.
#[derive(Debug, Clone)]
pub(super) struct ActivityState {
    pub(super) enabled: bool,
    pub(super) source: NetworkActivitySource,
    /// The last result, for the network and source it was read for.
    pub(super) shown: Option<Shown>,
    request: u64,
    /// The read under way: its request id, network and source.
    in_flight: Option<(u64, ChainId, NetworkActivitySource)>,
    read_at: Option<Instant>,
    /// When the shown tip advanced; the rail slides for [`ARRIVAL`].
    pub(super) arrived: Option<Instant>,
    pub(super) menu: bool,
    /// The menu's network choice: Bitcoin Blake2b rather than Bitcoin, for
    /// this session. Applies only while Bitcoin Blake2b is listed.
    pub(super) blake2b: bool,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            enabled: true,
            source: NetworkActivitySource::Automatic,
            shown: None,
            request: 0,
            in_flight: None,
            read_at: None,
            arrived: None,
            menu: false,
            blake2b: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Shown {
    pub(super) chain: ChainId,
    pub(super) source: NetworkActivitySource,
    pub(super) result: Result<Snapshot, Unavailable>,
}

impl ActivityState {
    /// The device preferences, as saved. A file that cannot be read keeps the
    /// defaults: activity on, Automatic.
    pub(super) fn load(root: &CoincubeDirectory) -> Self {
        let mut state = Self::default();
        state.apply_preferences(root);
        state
    }

    pub(super) fn apply_preferences(&mut self, root: &CoincubeDirectory) {
        if let Ok(preferences) = GlobalSettings::load_preferences(&GlobalSettings::path(root)) {
            self.enabled = preferences.network_activity;
            self.source = preferences.network_activity_source;
        }
        if !self.enabled {
            self.menu = false;
        }
    }

    /// The result for `chain` under the current source, if one was read.
    pub(super) fn current(&self, chain: ChainId) -> Option<&Result<Snapshot, Unavailable>> {
        self.shown
            .as_ref()
            .filter(|shown| shown.chain == chain && shown.source == self.source)
            .map(|shown| &shown.result)
    }

    /// Whether to read `chain` now. A read under way for another network or
    /// source does not hold up the one wanted: starting it supersedes that
    /// read, whose reply is then dropped.
    fn due(&self, chain: ChainId, now: Instant) -> bool {
        self.in_flight
            .is_none_or(|(_, busy, source)| busy != chain || source != self.source)
            && (self.current(chain).is_none()
                || self
                    .read_at
                    .is_none_or(|at| now.saturating_duration_since(at) >= REFRESH))
    }

    /// Starts a read of `chain` under the current source; its request id.
    fn start(&mut self, chain: ChainId) -> u64 {
        self.request = self.request.wrapping_add(1);
        self.in_flight = Some((self.request, chain, self.source));
        self.request
    }

    /// Takes a reply. Only the latest read counts, and only while its network
    /// and source are still the ones wanted: a reply for a network the user
    /// has left would replace what is shown for the one they are on.
    fn finish(
        &mut self,
        request: u64,
        wanted: ChainId,
        result: Result<Snapshot, Unavailable>,
        now: Instant,
    ) {
        let Some((latest, chain, source)) = self.in_flight else {
            return;
        };
        if latest != request {
            return;
        }
        self.in_flight = None;
        if chain != wanted || source != self.source {
            return;
        }
        self.read_at = Some(now);
        let previous_tip = self
            .shown
            .as_ref()
            .filter(|shown| shown.chain == chain)
            .and_then(|shown| shown.result.as_ref().ok())
            .map(|snapshot| snapshot.tip);
        if let (Some(before), Ok(snapshot)) = (previous_tip, &result) {
            if snapshot.tip > before {
                self.arrived = Some(now);
            }
        }
        self.shown = Some(Shown {
            chain,
            source,
            result,
        });
    }
}

/// The network the activity follows: the filtered network, Testnet4 for the
/// test networks, and Bitcoin otherwise; with `blake2b`, the Bitcoin Blake2b
/// network of the same kind.
pub(super) fn chain_for(filter: NetworkFilter, blake2b: bool) -> ChainId {
    match (filter, blake2b) {
        (NetworkFilter::Network(chain), _) => chain,
        (NetworkFilter::Test, false) => ChainId::Testnet4,
        (NetworkFilter::Test, true) => ChainId::BitcoinBlake2bTestnet4,
        (NetworkFilter::All | NetworkFilter::Live, false) => ChainId::Bitcoin,
        (NetworkFilter::All | NetworkFilter::Live, true) => ChainId::BitcoinBlake2b,
    }
}

/// Blocks between halvings. Bitcoin Blake2b keeps Bitcoin's subsidy
/// schedule and ten-minute blocks; regtest halves every 150 blocks.
pub(super) fn halving_interval(chain: ChainId) -> u32 {
    if chain == ChainId::Regtest {
        150
    } else {
        210_000
    }
}

/// Blocks left until the next halving after `tip`.
pub(super) fn blocks_to_halving(chain: ChainId, tip: u32) -> u32 {
    let every = halving_interval(chain);
    every - tip % every
}

/// Month and year of the next halving, at ten-minute blocks from `today`.
pub(super) fn halving_month(blocks: u32, today: chrono::NaiveDate) -> String {
    let date = today + chrono::Duration::minutes(i64::from(blocks) * 10);
    date.format(&coincube_ui::locale::date_pattern("%b %Y", &date))
        .to_string()
}

/// The hashrate a difficulty implies at ten-minute blocks.
pub(super) fn hashrate(difficulty: f64) -> String {
    const UNITS: [&str; 9] = [
        "H/s", "kH/s", "MH/s", "GH/s", "TH/s", "PH/s", "EH/s", "ZH/s", "YH/s",
    ];
    let mut value = difficulty * 4_294_967_296.0 / 600.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Difficulty from a compact target, for an Esplora without `difficulty`.
pub(super) fn difficulty_from_bits(bits: u32) -> f64 {
    let exponent = (bits >> 24) as i32;
    let mantissa = f64::from(bits & 0x00ff_ffff);
    if mantissa == 0.0 {
        return 0.0;
    }
    // The genesis target, 0x1d00ffff, has difficulty 1.
    (f64::from(0x00ffffu32) / mantissa) * 256f64.powi(0x1d - exponent)
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A block height with thousands separators.
pub(super) fn block_label(height: u32) -> String {
    grouped(u64::from(height))
}

/// The mempool as blocks' worth of transactions.
pub(super) fn backlog_blocks(vsize: u64) -> f64 {
    vsize as f64 / BLOCK_VSIZE
}

fn fee_label(fee: f64) -> String {
    if fee >= 10.0 {
        format!("≈ {fee:.0} sat/vB")
    } else {
        format!("≈ {fee:.1} sat/vB")
    }
}

// Reading a network.

pub(super) async fn read(
    root: CoincubeDirectory,
    chain: ChainId,
    source: NetworkActivitySource,
) -> Result<Snapshot, Unavailable> {
    match source {
        NetworkActivitySource::Automatic => match local(&root, chain).await {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                tracing::debug!("Launcher activity: no local node for {chain}: {error}");
                public(chain).await.map_err(|error| {
                    tracing::debug!("Launcher activity: public explorer for {chain}: {error}");
                    automatic_unavailable(chain)
                })
            }
        },
        NetworkActivitySource::LocalNode => local(&root, chain).await.map_err(|error| {
            tracing::debug!("Launcher activity: no local node for {chain}: {error}");
            Unavailable::LocalNodeNotRunning
        }),
        NetworkActivitySource::Connect => {
            let Some((via, _)) = public_source(chain) else {
                return Err(Unavailable::NoSource);
            };
            public(chain).await.map_err(|error| {
                tracing::debug!("Launcher activity: public explorer for {chain}: {error}");
                Unavailable::Unreachable(via)
            })
        }
    }
}

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// The client for the managed node. Its requests carry the node's cookie, so
/// they never go through a proxy from the environment: `NO_PROXY` alone
/// decides whether loopback is bypassed, and it rarely lists 127.0.0.1.
fn node_http(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, String> {
    builder
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// COINCUBE | Connect's public Esplora proxy for `chain`. No account needed.
pub(super) fn connect_base(chain: ChainId) -> String {
    format!(
        "{}/api/v1/esplora/{}",
        crate::services::coincube_api_base_url().trim_end_matches('/'),
        crate::installer::connect_esplora_path(chain)
    )
}

#[derive(Debug, Deserialize)]
pub(super) struct EsploraBlock {
    height: u32,
    tx_count: u32,
    #[serde(default)]
    difficulty: Option<f64>,
    #[serde(default)]
    bits: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EsploraMempool {
    vsize: u64,
}

/// Esplora's answers; `/blocks` lists the newest block first.
pub(super) fn from_esplora(
    mut blocks: Vec<EsploraBlock>,
    mempool: EsploraMempool,
    fees: &HashMap<String, f64>,
    via: Via,
) -> Result<Snapshot, String> {
    blocks.sort_by_key(|block| block.height);
    let tip = blocks.last().ok_or("No blocks")?;
    let difficulty = tip
        .difficulty
        .or_else(|| tip.bits.map(difficulty_from_bits))
        .ok_or("No difficulty")?;
    let tip_height = tip.height;
    let skip = blocks.len().saturating_sub(RAIL_BLOCKS);
    Ok(Snapshot {
        tip: tip_height,
        tx_counts: blocks
            .iter()
            .skip(skip)
            .map(|block| block.tx_count)
            .collect(),
        mempool_vsize: mempool.vsize,
        next_block_fee: fees.get("1").copied(),
        difficulty,
        via,
    })
}

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: String,
) -> Result<T, String> {
    client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|e| e.to_string())?
        .json::<T>()
        .await
        .map_err(|e| e.to_string())
}

async fn public(chain: ChainId) -> Result<Snapshot, String> {
    let (via, base) = public_source(chain).ok_or("No public explorer for this network")?;
    let client = http()?;
    let (blocks, mempool, fees) = tokio::try_join!(
        get::<Vec<EsploraBlock>>(&client, format!("{base}/blocks")),
        get::<EsploraMempool>(&client, format!("{base}/mempool")),
        get::<HashMap<String, f64>>(&client, format!("{base}/fee-estimates")),
    )?;
    from_esplora(blocks, mempool, &fees, via)
}

struct NodeRpc {
    client: reqwest::Client,
    url: String,
    user: String,
    pass: String,
}

impl NodeRpc {
    /// A JSON-RPC batch: one result per call, in order; `null` for an error.
    async fn batch(&self, calls: &[(&str, Value)]) -> Result<Vec<Value>, String> {
        let body: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(id, (method, params))| {
                json!({"jsonrpc": "1.0", "id": id, "method": method, "params": params})
            })
            .collect();
        let replies: Vec<Value> = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.pass))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let mut results = vec![Value::Null; calls.len()];
        for reply in replies {
            let Some(id) = reply["id"].as_u64().map(|id| id as usize) else {
                continue;
            };
            if id < results.len() && reply["error"].is_null() {
                results[id] = reply["result"].clone();
            }
        }
        Ok(results)
    }
}

/// The managed node for `chain`, when it is running: its config names the
/// RPC port, and bitcoind deletes the cookie on a clean shutdown.
async fn local(root: &CoincubeDirectory, chain: ChainId) -> Result<Snapshot, String> {
    use crate::node::bitcoind::{
        internal_bitcoind_config_path, internal_bitcoind_cookie_path,
        internal_bitcoind_datadir_for, InternalBitcoindConfig, NodeChainFamily,
    };
    let dir = internal_bitcoind_datadir_for(root, NodeChainFamily::from_chain(chain));
    let network = chain.bitcoin_network();
    let config = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(&dir))
        .map_err(|e| e.to_string())?;
    let port = config
        .networks
        .get(&network)
        .ok_or("No managed node for this network")?
        .rpc_port;
    // A few bytes on local disk, read on the blocking pool.
    let cookie_path = internal_bitcoind_cookie_path(&dir, &network);
    let cookie = tokio::task::spawn_blocking(move || std::fs::read_to_string(cookie_path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "The managed node is not running".to_string())?;
    let (user, pass) = cookie
        .trim()
        .split_once(':')
        .ok_or("Unreadable node cookie")?;
    let rpc = NodeRpc {
        client: node_http(reqwest::Client::builder())?,
        url: format!("http://127.0.0.1:{port}/"),
        user: user.to_string(),
        pass: pass.to_string(),
    };
    let first = rpc
        .batch(&[
            ("getblockchaininfo", json!([])),
            ("getmempoolinfo", json!([])),
            ("estimatesmartfee", json!([1])),
        ])
        .await?;
    let snapshot = from_node(&first[0], &first[1], &first[2])?;
    let oldest = snapshot.tip.saturating_sub(RAIL_BLOCKS as u32 - 1);
    let hashes = rpc
        .batch(
            &(oldest..=snapshot.tip)
                .map(|height| ("getblockhash", json!([height])))
                .collect::<Vec<_>>(),
        )
        .await?;
    let headers = rpc
        .batch(
            &hashes
                .iter()
                .filter_map(|hash| hash.as_str())
                .map(|hash| ("getblockheader", json!([hash])))
                .collect::<Vec<_>>(),
        )
        .await?;
    let tx_counts = headers
        .iter()
        .filter_map(|header| header["nTx"].as_u64())
        .map(|count| count as u32)
        .collect();
    Ok(Snapshot {
        tx_counts,
        ..snapshot
    })
}

/// The node's chain, mempool and fee answers. A node still syncing is not a
/// source: its tip is in the past.
pub(super) fn from_node(info: &Value, mempool: &Value, fee: &Value) -> Result<Snapshot, String> {
    if info["initialblockdownload"].as_bool() != Some(false) {
        return Err("The local node is still syncing".into());
    }
    let tip = info["blocks"].as_u64().ok_or("No block height")? as u32;
    // Knots `29.5.1.knots20260508` reports a header-v2 (post-BLAKE2b-fork) tip's
    // work as `difficulty_blake2b`, the expected number of hashes per block,
    // and omits `difficulty` (`PushDifficulty`, `src/rpc/blockchain.cpp`). A
    // difficulty multiple is expected hashes / 2^32, so dividing keeps
    // [`hashrate`]'s maths valid for both.
    let difficulty = match info["difficulty"].as_f64() {
        Some(difficulty) => difficulty,
        None => info["difficulty_blake2b"].as_f64().ok_or("No difficulty")? / 4_294_967_296.0,
    };
    // `bytes` is the sum of virtual sizes. `feerate` is BTC/kvB; Bitcoin Core
    // answers a one-block target with its two-block estimate, the closest it
    // gives to "the next block".
    let mempool_vsize = mempool["bytes"].as_u64().unwrap_or(0);
    let next_block_fee = fee["feerate"].as_f64().map(|rate| rate * 100_000.0);
    Ok(Snapshot {
        tip,
        tx_counts: Vec::new(),
        mempool_vsize,
        next_block_fee,
        difficulty,
        via: Via::LocalNode,
    })
}

// Update and subscription.

impl Home {
    pub(super) fn activity_chain(&self) -> ChainId {
        let blake2b = self.catalog.activity.blake2b && self.activity_blake2b_offered();
        chain_for(self.catalog.filter, blake2b)
    }

    /// The menu offers Bitcoin Blake2b beside Bitcoin while the filter names
    /// no single network and the Blake2b network of that kind is listed.
    pub(super) fn activity_blake2b_offered(&self) -> bool {
        !matches!(self.catalog.filter, NetworkFilter::Network(_))
            && self
                .displayed_networks
                .contains(&chain_for(self.catalog.filter, true))
    }

    /// Activity shows on the Cubes floor only, never in the List.
    pub(super) fn activity_wanted(&self) -> bool {
        self.catalog.activity.enabled
            && super::launcher_catalog::catalog_visible(self)
            && super::launcher_catalog::project(self).floor_eligible(self.catalog.mode)
    }

    pub(super) fn update_activity(&mut self, message: ActivityMessage) -> Task<Message> {
        match message {
            ActivityMessage::Tick(now) => {
                let chain = self.activity_chain();
                if !self.activity_wanted() || !self.catalog.activity.due(chain, now) {
                    return Task::none();
                }
                let activity = &mut self.catalog.activity;
                let request = activity.start(chain);
                let source = activity.source;
                Task::perform(
                    read(self.datadir_path.clone(), chain, source),
                    move |result| {
                        Message::View(ViewMessage::Catalog(CatalogMessage::Activity(
                            ActivityMessage::Loaded {
                                request,
                                chain,
                                source,
                                result,
                            },
                        )))
                    },
                )
            }
            ActivityMessage::Loaded {
                request, result, ..
            } => {
                let wanted = self.activity_chain();
                self.catalog
                    .activity
                    .finish(request, wanted, result, Instant::now());
                Task::none()
            }
            ActivityMessage::Frame(now) => {
                let activity = &mut self.catalog.activity;
                if activity
                    .arrived
                    .is_some_and(|at| now.saturating_duration_since(at) >= ARRIVAL)
                {
                    activity.arrived = None;
                }
                Task::none()
            }
            ActivityMessage::Menu(open) => {
                self.catalog.activity.menu = open && self.catalog.activity.enabled;
                if self.catalog.activity.menu {
                    self.catalog.plan_menu = false;
                    self.catalog.key = super::launcher_catalog::KeyCard::default();
                }
                Task::none()
            }
            ActivityMessage::Blake2b(on) => {
                self.catalog.activity.blake2b = on && self.activity_blake2b_offered();
                // The next tick reads the chosen network at once: `due` is
                // true for a network that is not the one shown.
                self.update_activity(ActivityMessage::Tick(Instant::now()))
            }
        }
    }

    pub(super) fn activity_subscription(&self, foreground: bool) -> Subscription<Message> {
        if !foreground || !self.activity_wanted() {
            return Subscription::none();
        }
        let tick = iced::time::every(TICK).map(|now| {
            Message::View(ViewMessage::Catalog(CatalogMessage::Activity(
                ActivityMessage::Tick(now),
            )))
        });
        if self.catalog.activity.arrived.is_some() && !self.activity_reduces_motion() {
            let frames = iced::window::frames().map(|now| {
                Message::View(ViewMessage::Catalog(CatalogMessage::Activity(
                    ActivityMessage::Frame(now),
                )))
            });
            return Subscription::batch([tick, frames]);
        }
        tick
    }

    /// Reduced motion normally shows the List, where there is no activity;
    /// this keeps the slide off should the floor ever show under it.
    fn activity_reduces_motion(&self) -> bool {
        self.system_reduces_motion == Some(true)
    }
}

// The band in the sky.

/// One fact under the rail.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Fact {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) note: String,
}

/// What the floor draws above the Cubes, owned so the view can move it into
/// its `responsive` closure.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Band {
    /// Width and height of each block, and its opacity, oldest first.
    blocks: Vec<(f32, f32, f32)>,
    tip_color: [u8; 4],
    /// The mempool queue's width, when anything waits.
    queue: Option<f32>,
    pub(super) facts: Vec<Fact>,
    arrived: Option<Instant>,
    reduce_motion: bool,
}

/// Rail, rule and facts. The band is left out below this much sky.
const BAND_HEIGHT: f32 = 96.0;
const BAND_MARGIN: f32 = 20.0;
const RULE_WIDTH: f32 = 760.0;

pub(super) fn facts(chain: ChainId, snapshot: &Snapshot, today: chrono::NaiveDate) -> Vec<Fact> {
    let backlog = backlog_blocks(snapshot.mempool_vsize);
    let waiting = backlog >= EMPTY_BACKLOG;
    let to_halving = blocks_to_halving(chain, snapshot.tip);
    let mut facts = vec![
        Fact {
            label: "BLOCK",
            value: format!("#{}", block_label(snapshot.tip)),
            note: match snapshot.via {
                Via::LocalNode => "from your node".into(),
                Via::Connect => "from Connect".into(),
                Via::MempoolSpace => "from mempool.space".into(),
            },
        },
        Fact {
            label: "MEMPOOL",
            value: if waiting {
                format!("≈ {backlog:.1} blocks")
            } else {
                "empty".into()
            },
            note: if waiting {
                "worth of txs waiting to confirm".into()
            } else {
                "nothing waiting".into()
            },
        },
    ];
    if let Some(fee) = snapshot.next_block_fee {
        facts.push(Fact {
            label: "FEE RATE",
            value: fee_label(fee),
            note: "to make the next block".into(),
        });
    }
    // Only the main chains have a difficulty that says what is mining them.
    // Signet blocks are signed, regtest's cost nothing to mine, and testnet's
    // 20-minute rule often leaves the tip at the minimum difficulty.
    if matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b) {
        facts.push(Fact {
            label: "HASHRATE",
            value: hashrate(snapshot.difficulty),
            note: "from difficulty".into(),
        });
    }
    facts.push(Fact {
        label: "NEXT HALVING",
        value: if chain == ChainId::Regtest {
            format!("block {}", block_label(snapshot.tip + to_halving))
        } else {
            format!("≈ {}", halving_month(to_halving, today))
        },
        note: format!("in {} blocks", block_label(to_halving)),
    });
    facts
}

/// Block bars: width and height grow with the block's transaction count,
/// and older blocks fade.
pub(super) fn bars(tx_counts: &[u32]) -> Vec<(f32, f32, f32)> {
    let most = tx_counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    let count = tx_counts.len().max(1) as f32;
    let last = tx_counts.len().saturating_sub(1);
    tx_counts
        .iter()
        .enumerate()
        .map(|(index, &txs)| {
            let share = txs as f32 / most;
            let age = (last - index) as f32 / count;
            (16.0 + 14.0 * share, 5.0 + 5.0 * share, 0.9 - 0.6 * age)
        })
        .collect()
}

impl Home {
    /// The band for the current network, once a read succeeded.
    pub(super) fn activity_band(&self, light: bool) -> Option<Band> {
        if !self.activity_wanted() {
            return None;
        }
        let chain = self.activity_chain();
        let snapshot = self.catalog.activity.current(chain)?.as_ref().ok()?;
        let backlog = backlog_blocks(snapshot.mempool_vsize);
        Some(Band {
            blocks: bars(&snapshot.tx_counts),
            tip_color: launcher_scene::edge(chain, light),
            queue: (backlog >= EMPTY_BACKLOG).then(|| (backlog as f32 * 24.0).clamp(4.0, 160.0)),
            facts: facts(chain, snapshot, chrono::Local::now().date_naive()),
            arrived: self.catalog.activity.arrived,
            reduce_motion: self.activity_reduces_motion(),
        })
    }
}

fn rgba(color: [u8; 4]) -> Color {
    Color::from_rgba8(color[0], color[1], color[2], f32::from(color[3]) / 255.0)
}

fn fill(color: Color) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(color)),
        border: Border {
            radius: 1.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn fact<'a, M: 'a>(fact: &Fact) -> Element<'a, M> {
    let muted = |theme: &theme::Theme| text::Style {
        color: Some(theme.colors.shell.muted),
    };
    Column::new()
        .spacing(2)
        .align_x(Alignment::Center)
        .push(text(fact.label).size(11).font(font::REGULAR).style(muted))
        .push(
            text(fact.value.clone())
                .size(13)
                .font(font::MONO_REGULAR)
                .style(|theme: &theme::Theme| text::Style {
                    color: Some(theme.colors.shell.ink),
                }),
        )
        .push(
            text(fact.note.clone())
                .size(11)
                .font(font::REGULAR)
                .style(muted),
        )
        .into()
}

/// The band laid over the stage's sky, above Cubes whose tops are `sky`
/// pixels down the stage. With too little clear sky it is left out.
pub(super) fn band_layer<'a, M: 'a>(band: &Band, sky: f32, now: Instant) -> Option<Element<'a, M>> {
    if sky < BAND_HEIGHT + 2.0 * BAND_MARGIN {
        return None;
    }
    let top = ((sky - BAND_HEIGHT) / 2.0).max(BAND_MARGIN);
    let shift = match band.arrived {
        Some(at) if !band.reduce_motion => {
            let t = (now.saturating_duration_since(at).as_secs_f32() / ARRIVAL.as_secs_f32())
                .clamp(0.0, 1.0);
            (1.0 - launcher_scene::ease(t)) * ARRIVAL_SHIFT
        }
        _ => 0.0,
    };
    let last = band.blocks.len().saturating_sub(1);
    let tip_color = rgba(band.tip_color);
    // A leading space twice the shift moves the centred rail by the shift.
    let mut rail = Row::new()
        .spacing(3)
        .align_y(Alignment::End)
        .push(Space::new().width(Length::Fixed(2.0 * shift)));
    for (index, &(width, height, alpha)) in band.blocks.iter().enumerate() {
        let tip = index == last;
        rail = rail.push(
            Container::new(Space::new())
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .style(move |theme: &theme::Theme| {
                    fill(if tip {
                        tip_color
                    } else {
                        let mut line = theme.colors.shell.line;
                        line.a *= alpha;
                        line
                    })
                }),
        );
    }
    if let Some(queue) = band.queue {
        rail = rail.push(Space::new().width(Length::Fixed(5.0))).push(
            Container::new(Space::new())
                .width(Length::Fixed(queue))
                .height(Length::Fixed(6.0))
                .style(|theme: &theme::Theme| container::Style {
                    border: Border {
                        color: theme.colors.shell.line,
                        width: 1.0,
                        radius: 1.0.into(),
                    },
                    ..Default::default()
                }),
        );
    }
    let mut facts = Row::new().spacing(40).align_y(Alignment::Start);
    for item in &band.facts {
        facts = facts.push(fact(item));
    }
    Some(
        Container::new(
            Column::new()
                .align_x(Alignment::Center)
                .push(
                    Container::new(rail)
                        .height(Length::Fixed(12.0))
                        .align_y(iced::alignment::Vertical::Bottom),
                )
                .push(
                    Container::new(Space::new())
                        .width(Length::Fixed(RULE_WIDTH))
                        .height(Length::Fixed(1.0))
                        .style(|theme: &theme::Theme| fill(theme.colors.shell.line)),
                )
                .push(Space::new().height(Length::Fixed(14.0)))
                .push(facts),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding {
            top,
            ..Padding::ZERO
        })
        .into(),
    )
}

#[cfg(test)]
impl ActivityState {
    /// A finished read, as if the source had answered.
    pub(super) fn show_for_test(&mut self, chain: ChainId, result: Result<Snapshot, Unavailable>) {
        self.shown = Some(Shown {
            chain,
            source: self.source,
            result,
        });
        self.read_at = Some(Instant::now());
    }
}

/// Bitcoin mainnet as the mock-ups show it, read from Connect.
#[cfg(test)]
pub(super) fn sample_snapshot() -> Snapshot {
    Snapshot {
        tip: 968_412,
        tx_counts: vec![3100, 2900, 3400, 2000, 3550, 3000, 2700, 3380, 3600, 2450],
        mempool_vsize: 3_400_000,
        next_block_fee: Some(6.2),
        difficulty: 150e12,
        via: Via::Connect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(tip: u32) -> Snapshot {
        Snapshot {
            tip,
            tx_counts: vec![3000; RAIL_BLOCKS],
            mempool_vsize: 3_400_000,
            next_block_fee: Some(6.2),
            difficulty: 150e12,
            via: Via::Connect,
        }
    }

    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 28).unwrap()
    }

    #[test]
    fn follows_the_network_filter() {
        assert_eq!(chain_for(NetworkFilter::All, false), ChainId::Bitcoin);
        assert_eq!(chain_for(NetworkFilter::Live, false), ChainId::Bitcoin);
        assert_eq!(chain_for(NetworkFilter::Test, false), ChainId::Testnet4);
        assert_eq!(
            chain_for(NetworkFilter::Network(ChainId::BitcoinBlake2b), false),
            ChainId::BitcoinBlake2b
        );
    }

    #[test]
    fn the_blake2b_choice_picks_the_same_kind_unless_one_network_is_filtered() {
        assert_eq!(chain_for(NetworkFilter::All, true), ChainId::BitcoinBlake2b);
        assert_eq!(
            chain_for(NetworkFilter::Live, true),
            ChainId::BitcoinBlake2b
        );
        assert_eq!(
            chain_for(NetworkFilter::Test, true),
            ChainId::BitcoinBlake2bTestnet4
        );
        assert_eq!(
            chain_for(NetworkFilter::Network(ChainId::Bitcoin), true),
            ChainId::Bitcoin
        );
    }

    #[test]
    fn counts_blocks_to_the_next_halving() {
        assert_eq!(blocks_to_halving(ChainId::Bitcoin, 968_412), 81_588);
        assert_eq!(blocks_to_halving(ChainId::Bitcoin, 840_000), 210_000);
        assert_eq!(blocks_to_halving(ChainId::BitcoinBlake2b, 951_806), 98_194);
        assert_eq!(blocks_to_halving(ChainId::Regtest, 412), 38);
        assert_eq!(halving_month(81_588, today()), "May 2028");
    }

    #[test]
    fn hashrate_comes_from_difficulty() {
        // Difficulty 1 is 2^32 hashes a block: about 7.16 MH/s.
        assert_eq!(hashrate(1.0), "7.16 MH/s");
        assert_eq!(hashrate(150e12), "1.07 ZH/s");
        assert_eq!(hashrate(1.5e12), "10.7 EH/s");
        assert!((difficulty_from_bits(0x1d00ffff) - 1.0).abs() < 1e-9);
        // Block 840,000's target and its published difficulty.
        let d = difficulty_from_bits(0x17034219);
        assert!((d - 86_388_558_925_171.0).abs() / d < 1e-6, "{}", d);
    }

    #[test]
    fn reads_esplora_answers_oldest_first() {
        let blocks: Vec<EsploraBlock> = serde_json::from_value(json!([
            {"id": "c", "height": 102, "tx_count": 30, "difficulty": 2.0},
            {"id": "b", "height": 101, "tx_count": 20, "bits": 486604799u32},
            {"id": "a", "height": 100, "tx_count": 10, "difficulty": 1.0},
        ]))
        .unwrap();
        let mempool: EsploraMempool =
            serde_json::from_value(json!({"count": 9, "vsize": 2_200_000, "total_fee": 1}))
                .unwrap();
        let fees: HashMap<String, f64> =
            serde_json::from_value(json!({"1": 6.5, "6": 3.0})).unwrap();
        let snapshot = from_esplora(blocks, mempool, &fees, Via::Connect).unwrap();
        assert_eq!(snapshot.tip, 102);
        assert_eq!(snapshot.tx_counts, vec![10, 20, 30]);
        assert_eq!(snapshot.mempool_vsize, 2_200_000);
        assert_eq!(snapshot.next_block_fee, Some(6.5));
        assert_eq!(snapshot.difficulty, 2.0);
        assert_eq!(snapshot.via, Via::Connect);
    }

    #[test]
    fn an_esplora_tip_without_difficulty_uses_its_bits() {
        let blocks: Vec<EsploraBlock> =
            serde_json::from_value(json!([{"height": 7, "tx_count": 1, "bits": 486604799u32}]))
                .unwrap();
        let snapshot = from_esplora(
            blocks,
            EsploraMempool { vsize: 0 },
            &HashMap::new(),
            Via::MempoolSpace,
        )
        .unwrap();
        assert!((snapshot.difficulty - 1.0).abs() < 1e-9);
        assert_eq!(snapshot.next_block_fee, None);
        assert!(from_esplora(
            Vec::new(),
            EsploraMempool { vsize: 0 },
            &HashMap::new(),
            Via::Connect
        )
        .is_err());
    }

    #[test]
    fn a_syncing_node_is_not_a_source() {
        let syncing = json!({"blocks": 5, "difficulty": 1.0, "initialblockdownload": true});
        assert!(from_node(&syncing, &json!({}), &json!({})).is_err());
        let synced = json!({"blocks": 968_412, "difficulty": 1.0, "initialblockdownload": false});
        let snapshot = from_node(
            &synced,
            &json!({"size": 14_300, "bytes": 3_400_000}),
            &json!({"feerate": 0.00006, "blocks": 2}),
        )
        .unwrap();
        assert_eq!(snapshot.tip, 968_412);
        assert_eq!(snapshot.mempool_vsize, 3_400_000);
        assert!((snapshot.next_block_fee.unwrap() - 6.0).abs() < 1e-9);
        assert_eq!(snapshot.via, Via::LocalNode);
        // Regtest's estimatesmartfee answers with errors and no feerate.
        let no_fee = from_node(&synced, &json!({"bytes": 0}), &json!({"errors": ["x"]})).unwrap();
        assert_eq!(no_fee.next_block_fee, None);
    }

    #[test]
    fn a_blake2b_tip_reports_its_work_as_expected_hashes() {
        // A header-v2 tip carries `difficulty_blake2b` (expected hashes per
        // block) and no `difficulty`; 600 TH per block is 1 TH/s.
        let info = json!({"blocks": 977_000, "difficulty_blake2b": 6.0e14,
            "initialblockdownload": false});
        let snapshot = from_node(&info, &json!({"bytes": 0}), &json!({})).unwrap();
        assert!((snapshot.difficulty * 4_294_967_296.0 - 6.0e14).abs() < 1.0);
        assert_eq!(hashrate(snapshot.difficulty), "1.00 TH/s");
        // Neither field: still an error, not a zero hashrate.
        let none = json!({"blocks": 977_000, "initialblockdownload": false});
        assert!(from_node(&none, &json!({}), &json!({})).is_err());
    }

    #[test]
    fn connect_serves_blake2b_from_its_public_proxy() {
        assert!(connect_base(ChainId::BitcoinBlake2b)
            .ends_with("/api/v1/esplora/bitcoin-blake2b/mainnet"));
        assert!(connect_base(ChainId::BitcoinBlake2bTestnet4)
            .ends_with("/api/v1/esplora/bitcoin-blake2b/testnet4"));
        assert!(connect_base(ChainId::Bitcoin).ends_with("/api/v1/esplora/bitcoin/mainnet"));
    }

    #[test]
    fn each_network_reads_an_explorer_that_serves_it() {
        // Connect's Esplora proxy serves Bitcoin and both Blake2b networks;
        // it answers 404 for testnet4 and signet.
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            let (via, base) = public_source(chain).unwrap();
            assert_eq!(via, Via::Connect);
            assert_eq!(base, connect_base(chain));
        }
        for (chain, base) in [
            (ChainId::Testnet, "https://mempool.space/testnet/api"),
            (ChainId::Testnet4, "https://mempool.space/testnet4/api"),
            (ChainId::Signet, "https://mempool.space/signet/api"),
        ] {
            assert_eq!(
                public_source(chain),
                Some((Via::MempoolSpace, base.to_string()))
            );
        }
        assert_eq!(public_source(ChainId::Regtest), None);
        assert_eq!(
            Unavailable::Unreachable(Via::MempoolSpace).label(),
            "mempool.space is unreachable"
        );
        // Automatic names the explorer that failed after the local node,
        // and on regtest, which has no explorer, the node it needs.
        assert_eq!(
            automatic_unavailable(ChainId::Bitcoin).label(),
            "Local node and Connect didn't answer"
        );
        assert_eq!(
            automatic_unavailable(ChainId::Signet).label(),
            "Local node and mempool.space didn't answer"
        );
        assert_eq!(
            automatic_unavailable(ChainId::Regtest),
            Unavailable::LocalNodeNotRunning
        );
        let mut testnet = snapshot(120_000);
        testnet.via = Via::MempoolSpace;
        let block = &facts(ChainId::Testnet4, &testnet, today())[0];
        assert_eq!(block.note, "from mempool.space");
    }

    #[test]
    fn facts_read_in_order() {
        let facts = facts(ChainId::Bitcoin, &snapshot(968_412), today());
        let labels: Vec<_> = facts.iter().map(|f| f.label).collect();
        assert_eq!(
            labels,
            ["BLOCK", "MEMPOOL", "FEE RATE", "HASHRATE", "NEXT HALVING"]
        );
        assert_eq!(facts[0].value, "#968,412");
        assert_eq!(facts[0].note, "from Connect");
        assert_eq!(facts[1].value, "≈ 3.4 blocks");
        assert_eq!(facts[1].note, "worth of txs waiting to confirm");
        assert_eq!(facts[2].value, "≈ 6.2 sat/vB");
        assert_eq!(facts[2].note, "to make the next block");
        assert_eq!(facts[3].value, "1.07 ZH/s");
        assert_eq!(facts[4].value, "≈ May 2028");
        assert_eq!(facts[4].note, "in 81,588 blocks");
    }

    #[test]
    fn only_main_chains_show_a_hashrate_and_regtest_names_the_halving_block() {
        for chain in [
            ChainId::Signet,
            ChainId::Testnet4,
            ChainId::Testnet,
            ChainId::Regtest,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            let shown = facts(chain, &snapshot(281_044), today());
            assert!(
                shown.iter().all(|f| f.label != "HASHRATE"),
                "{:?} shows a hashrate",
                chain
            );
        }
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let shown = facts(chain, &snapshot(281_044), today());
            assert!(shown.iter().any(|f| f.label == "HASHRATE"));
        }
        let regtest = facts(ChainId::Regtest, &snapshot(412), today());
        assert_eq!(regtest.last().unwrap().value, "block 450");
        assert_eq!(regtest.last().unwrap().note, "in 38 blocks");
    }

    #[test]
    fn an_empty_mempool_says_so() {
        let mut quiet = snapshot(100);
        quiet.mempool_vsize = 0;
        quiet.next_block_fee = None;
        let facts = facts(ChainId::Testnet4, &quiet, today());
        assert_eq!(facts[1].value, "empty");
        assert_eq!(facts[1].note, "nothing waiting");
        assert!(facts.iter().all(|f| f.label != "FEE RATE"));
    }

    #[test]
    fn bars_grow_with_transactions_and_fade_with_age() {
        let bars = bars(&[1000, 2000, 4000]);
        assert_eq!(bars.len(), 3);
        assert!(bars[0].0 < bars[1].0 && bars[1].0 < bars[2].0);
        assert!(bars[0].2 < bars[2].2);
        assert_eq!(bars[2], (30.0, 10.0, 0.9));
        assert!(super::bars(&[]).is_empty());
    }

    #[test]
    fn reads_again_only_when_due() {
        let now = Instant::now();
        let mut state = ActivityState::default();
        assert!(state.due(ChainId::Bitcoin, now));
        state.shown = Some(Shown {
            chain: ChainId::Bitcoin,
            source: NetworkActivitySource::Automatic,
            result: Ok(snapshot(1)),
        });
        state.read_at = Some(now);
        assert!(!state.due(ChainId::Bitcoin, now + Duration::from_secs(5)));
        assert!(state.due(ChainId::Bitcoin, now + REFRESH));
        // Another network or source has nothing shown yet.
        assert!(state.due(ChainId::BitcoinBlake2b, now));
        state.source = NetworkActivitySource::Connect;
        assert!(state.due(ChainId::Bitcoin, now));
        assert!(state.current(ChainId::Bitcoin).is_none());
        state.start(ChainId::Bitcoin);
        assert!(!state.due(ChainId::Bitcoin, now));
    }

    #[test]
    fn a_new_network_supersedes_the_read_under_way() {
        let now = Instant::now();
        let mut state = ActivityState::default();
        // Bitcoin is shown; a Blake2b read starts and the user goes back.
        let first = state.start(ChainId::Bitcoin);
        state.finish(first, ChainId::Bitcoin, Ok(snapshot(1)), now);
        let blake = state.start(ChainId::BitcoinBlake2b);
        assert!(state.due(ChainId::Testnet4, now));
        assert!(!state.due(ChainId::BitcoinBlake2b, now));
        state.finish(blake, ChainId::Bitcoin, Ok(snapshot(9)), now);
        assert_eq!(
            state.current(ChainId::Bitcoin),
            Some(&Ok(snapshot(1))),
            "a reply for a network the user left must not replace the shown one"
        );
        assert!(state.in_flight.is_none());

        // A slow read is superseded by a new network; its late reply is dropped.
        let slow = state.start(ChainId::Bitcoin);
        let testnet = state.start(ChainId::Testnet4);
        state.finish(slow, ChainId::Testnet4, Ok(snapshot(5)), now);
        assert!(state.current(ChainId::Testnet4).is_none());
        state.finish(testnet, ChainId::Testnet4, Ok(snapshot(7)), now);
        assert_eq!(state.current(ChainId::Testnet4), Some(&Ok(snapshot(7))));
    }

    #[tokio::test]
    async fn the_node_client_ignores_proxies() {
        // A listener stands in for a proxy the environment would name. The
        // request goes to a closed loopback port, never to the listener.
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let address = format!("http://{}", proxy.local_addr().unwrap());
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let builder = reqwest::Client::builder().proxy(reqwest::Proxy::all(&address).unwrap());
        let rpc = NodeRpc {
            client: node_http(builder).unwrap(),
            url: format!("http://127.0.0.1:{}/", port),
            user: "__cookie__".into(),
            pass: "secret".into(),
        };
        assert!(rpc
            .batch(&[("getblockchaininfo", json!([]))])
            .await
            .is_err());
        assert!(
            proxy.accept().is_err(),
            "the node's cookie must never reach a proxy"
        );
    }

    #[test]
    fn the_band_needs_clear_sky() {
        let band = Band {
            blocks: vec![(20.0, 8.0, 0.9)],
            tip_color: [243, 130, 29, 153],
            queue: Some(40.0),
            facts: Vec::new(),
            arrived: None,
            reduce_motion: false,
        };
        let now = Instant::now();
        assert!(band_layer::<()>(&band, BAND_HEIGHT, now).is_none());
        assert!(band_layer::<()>(&band, BAND_HEIGHT + 2.0 * BAND_MARGIN, now).is_some());
    }
}
