//! Bounded network discovery, retaining raw evidence without authorizing a spend.
use super::*;
pub(crate) mod diagnostics;
mod prune;
use coincube_core::{
    claim_ancestry::{
        self,
        search::{OwnedLink, Search, Step},
    },
    miniscript::bitcoin::OutPoint,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

// A pair with several shared Vault coins rechecks each coin on both chains in
// addition to the two funding paths. Keep a finite cap alongside the shared
// wall-clock and response-byte limits, but allow an ordinary small Vault to
// finish its complete proof instead of truncating it after 128 reads.
const MAX_REQUESTS: usize = 256;
const MAX_DISCOVERY_REQUESTS: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DISCOVERY_TIME: Duration = Duration::from_secs(90);

pub(super) struct CollectionBudget {
    requests: AtomicUsize,
    bytes: AtomicUsize,
}
impl CollectionBudget {
    fn new() -> Self {
        Self::with_requests(MAX_REQUESTS)
    }
    fn for_discovery() -> Self {
        Self::with_requests(MAX_DISCOVERY_REQUESTS)
    }
    fn with_requests(requests: usize) -> Self {
        Self {
            requests: AtomicUsize::new(requests),
            bytes: AtomicUsize::new(MAX_RESPONSE_BYTES),
        }
    }
    pub(super) fn charge_request(&self, reserved_bytes: usize) -> Result<(), FailureKind> {
        self.requests
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .map_err(|_| FailureKind::CollectionLimit)?;
        self.charge_bytes(reserved_bytes)
    }
    pub(super) fn charge_bytes(&self, bytes: usize) -> Result<(), FailureKind> {
        self.bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_sub(bytes)
            })
            .map_err(|_| FailureKind::CollectionLimit)?;
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryError {
    Structural(claim_ancestry::Error),
    Observation(FailureKind),
}

#[derive(Debug)]
pub struct DiscoveredAncestry {
    links: Vec<OwnedLink>,
    pair: CoinbasePair,
    observed_at: i64,
}
impl DiscoveredAncestry {
    /// Retain raw structural evidence only. Restoring it must use the intent's
    /// selected outpoint and obtain new chain observations before any spend.
    pub fn retained_path(
        &self,
    ) -> Result<claim_ancestry::retained::RetainedPath, claim_ancestry::Error> {
        claim_ancestry::retained::RetainedPath::new(self.pair.selected(), self.links.clone())
    }
    pub fn links(&self) -> &[OwnedLink] {
        &self.links
    }
    pub fn pair(&self) -> &CoinbasePair {
        &self.pair
    }
    pub fn observed_at(&self) -> i64 {
        self.observed_at
    }
    pub fn generation(&self) -> u64 {
        self.pair.generation()
    }
}
impl HttpObservationSource {
    /// Copy provider/session context and the diagnostic observer for one read.
    /// An operation's first snapshot allocates its bounded collection budget;
    /// nested and parallel snapshots share the existing request/byte counters
    /// rather than multiplying allowances. This does not refresh evidence.
    fn discovery_snapshot(&self) -> Self {
        Self {
            authenticated: self.authenticated.clone(),
            anonymous: self.anonymous.clone(),
            base: self.base.clone(),
            generation: self.generation.clone(),
            expected: self.expected,
            budget: Some(
                self.budget
                    .clone()
                    .unwrap_or_else(|| Arc::new(CollectionBudget::new())),
            ),
            ancestry_trace: self.ancestry_trace.clone(),
            ancestry_timing_observer: self.ancestry_timing_observer.clone(),
            #[cfg(all(test, feature = "regtest-harness"))]
            test_ancestry_history: self.test_ancestry_history,
        }
    }
    /// Wider search allowances apply only to a new discovery operation.
    /// Nested snapshots continue sharing the original finite counters.
    fn discovery_search_snapshot(&self) -> Self {
        let mut snapshot = self.discovery_snapshot();
        if self.budget.is_none() {
            snapshot.budget = Some(Arc::new(CollectionBudget::for_discovery()));
        }
        if snapshot.ancestry_trace.is_none() {
            snapshot.ancestry_trace = Some(Arc::new(diagnostics::Trace::new(
                snapshot.ancestry_timing_observer.clone(),
            )));
        }
        snapshot
    }
    async fn qualify_dependency(
        &self,
        dependency: &claim_ancestry::CoinbaseDependency,
        policy: Policy,
        root_chain: ChainId,
    ) -> Result<(CoinbasePair, i64), FailureKind> {
        if !matches!(root_chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b) {
            return Err(FailureKind::WrongChain);
        }
        let (root, stamp) = super::super::read(
            self.transaction(root_chain, dependency.root().txid).await?,
            root_chain,
            self,
            policy,
            Stage::BitcoinInclusion,
        )
        .map_err(|error| error.kind)?;
        let TransactionObservation::Confirmed { block, .. } = root else {
            return Err(FailureKind::Changed);
        };
        let height = u32::try_from(block.height).map_err(|_| FailureKind::Malformed)?;
        let pair = match root_chain {
            ChainId::Bitcoin => self.coinbase_pair(dependency, height, policy).await?,
            ChainId::BitcoinBlake2b => self.fork_coinbase_pair(dependency, height, policy).await?,
            _ => return Err(FailureKind::WrongChain),
        };
        let canonical_block = match root_chain {
            ChainId::Bitcoin => pair.bitcoin().block,
            ChainId::BitcoinBlake2b => pair.fork().block,
            _ => return Err(FailureKind::WrongChain),
        };
        if canonical_block != block {
            return Err(FailureKind::Changed);
        }
        let oldest = pair.observed_at().min(stamp);
        if !super::super::fresh(oldest, self.now(), policy.max_observation_age_seconds) {
            return Err(FailureKind::Stale);
        }
        Ok((pair, oldest))
    }
    /// Reverify a restored raw path and obtain entirely new chain observations.
    /// Decode the record against the intent's selected outpoint first. This does
    /// not check ownership, maturity or spendability and cannot authorize spending.
    pub async fn requalify_ancestry(
        &self,
        path: &claim_ancestry::retained::RetainedPath,
        policy: Policy,
    ) -> Result<DiscoveredAncestry, DiscoveryError> {
        self.requalify_ancestry_for(path, policy, ChainId::Bitcoin)
            .await
    }

    /// Refresh a retained BTCB2 path against both canonical chains. This is
    /// structural evidence only; it does not authorize signing or submission.
    pub async fn requalify_fork_ancestry(
        &self,
        path: &claim_ancestry::retained::RetainedPath,
        policy: Policy,
    ) -> Result<DiscoveredAncestry, DiscoveryError> {
        self.requalify_ancestry_for(path, policy, ChainId::BitcoinBlake2b)
            .await
    }

    async fn requalify_ancestry_for(
        &self,
        path: &claim_ancestry::retained::RetainedPath,
        policy: Policy,
        root_chain: ChainId,
    ) -> Result<DiscoveredAncestry, DiscoveryError> {
        if policy.max_observation_age_seconds <= 0 {
            return Err(DiscoveryError::Observation(FailureKind::InvalidPlan));
        }
        let dependency = path.reverify().map_err(DiscoveryError::Structural)?;
        let snapshot = self.discovery_snapshot();
        let (pair, observed_at) = tokio::time::timeout(
            MAX_COLLECTION_TIME,
            snapshot.qualify_dependency(&dependency, policy, root_chain),
        )
        .await
        .map_err(|_| {
            if let Some(trace) = &snapshot.ancestry_trace {
                trace.emit(root_chain, diagnostics::Outcome::CollectionDeadline30s);
            }
            DiscoveryError::Observation(FailureKind::Deadline)
        })?
        .map_err(DiscoveryError::Observation)?;
        if !session_current(&snapshot.generation, snapshot.expected) {
            return Err(DiscoveryError::Observation(FailureKind::Cancelled));
        }
        Ok(DiscoveredAncestry {
            links: path.links().to_vec(),
            pair,
            observed_at,
        })
    }
    /// Discover one qualifying mainnet dependency with shared graph, response,
    /// request and 90s search limits. None means no qualifying candidate was found,
    /// never proof of replay safety. Persisted links require re-verification;
    /// ownership, maturity, spend policy and final fresh preflight remain separate.
    pub async fn discover_ancestry(
        &self,
        selected: OutPoint,
        policy: Policy,
    ) -> Result<Option<DiscoveredAncestry>, DiscoveryError> {
        self.discover_ancestry_for(selected, policy, ChainId::Bitcoin)
            .await
    }

    /// Search a BTCB2 transaction graph for a positive post-fork coinbase
    /// dependency. Candidate ownership, maturity and spendability are separate.
    pub async fn discover_fork_ancestry(
        &self,
        selected: OutPoint,
        policy: Policy,
    ) -> Result<Option<DiscoveredAncestry>, DiscoveryError> {
        self.discover_ancestry_for(selected, policy, ChainId::BitcoinBlake2b)
            .await
    }

    async fn discover_ancestry_for(
        &self,
        selected: OutPoint,
        policy: Policy,
        root_chain: ChainId,
    ) -> Result<Option<DiscoveredAncestry>, DiscoveryError> {
        if policy.max_observation_age_seconds <= 0 {
            return Err(DiscoveryError::Observation(FailureKind::InvalidPlan));
        }
        let mut search = Search::new(selected).map_err(DiscoveryError::Structural)?;
        let standalone = self.ancestry_trace.is_none();
        let snapshot = self.discovery_search_snapshot();
        let trace = snapshot.ancestry_trace.as_ref().unwrap();
        let observation = DiscoveryError::Observation;
        let result = tokio::time::timeout(MAX_DISCOVERY_TIME, async {
            let mut pruning =
                prune::PruneContext::new(&snapshot, root_chain).map_err(observation)?;
            loop {
                match search.step().map_err(DiscoveryError::Structural)? {
                    Step::Fetch { txid, max_bytes } => {
                        trace.stage(root_chain, diagnostics::Stage::TransactionReads);
                        // Both reads concern this same node and fit the existing
                        // shared request/byte budget. Keep raw-read error
                        // precedence and perform canonical pruning only after
                        // both bounded reads have finished.
                        let (raw, transaction) = tokio::join!(
                            snapshot.ancestry_transaction_search(root_chain, txid, max_bytes),
                            snapshot.transaction(root_chain, txid)
                        );
                        let raw =
                            raw.map_err(|error| discovery_raw_read_error(error, max_bytes))?;
                        let transaction = transaction.map_err(observation)?;
                        trace.stage(root_chain, diagnostics::Stage::PreForkCheck);
                        let pre_fork = pruning
                            .inspect_prefetched(&snapshot, txid, policy, transaction)
                            .await
                            .map_err(observation)?;
                        search
                            .provide_with_pruning(raw, pre_fork)
                            .map_err(DiscoveryError::Structural)?;
                        trace.visited(root_chain, pre_fork);
                    }
                    Step::Candidate(candidate) => {
                        trace.stage(root_chain, diagnostics::Stage::RootQualification);
                        trace.root(root_chain);
                        match snapshot
                            .qualify_dependency(&candidate.dependency, policy, root_chain)
                            .await
                        {
                            // Only positively disqualified roots allow another branch.
                            Err(FailureKind::UnsupportedPoison) => {
                                trace.disqualified_root(root_chain, false);
                                continue;
                            }
                            Err(FailureKind::AncestryRootShared { .. }) => {
                                trace.disqualified_root(root_chain, true);
                                continue;
                            }
                            Err(error) => return Err(observation(error)),
                            Ok((pair, observed_at)) => {
                                return Ok(Some(DiscoveredAncestry {
                                    links: candidate.links,
                                    pair,
                                    observed_at,
                                }))
                            }
                        }
                    }
                    Step::Complete => {
                        trace.stage(root_chain, diagnostics::Stage::FrontierRecheck);
                        pruning
                            .finish(&snapshot, policy)
                            .await
                            .map_err(observation)?;
                        return Ok(None);
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            trace.deadline_if_active(root_chain);
            observation(FailureKind::Deadline)
        })?;
        if !session_current(&snapshot.generation, snapshot.expected) {
            return Err(observation(FailureKind::Cancelled));
        }
        if standalone {
            trace.complete(
                root_chain,
                match &result {
                    Ok(Some(_)) => diagnostics::Outcome::PathFound,
                    Ok(None) => diagnostics::Outcome::NoQualifyingAncestry,
                    Err(_) => diagnostics::Outcome::SearchFailed,
                },
            );
        }
        result
    }
}

fn discovery_raw_read_error(
    (error, exceeded_graph_bytes): (FailureKind, bool),
    max_bytes: usize,
) -> DiscoveryError {
    if exceeded_graph_bytes && max_bytes < claim_ancestry::MAX_TRANSACTION_BYTES {
        DiscoveryError::Structural(claim_ancestry::Error::ByteLimit)
    } else {
        DiscoveryError::Observation(error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{ancestry_fixture, source as make_source};
    use super::*;
    use coincube_core::miniscript::bitcoin::{
        absolute,
        consensus::{deserialize, serialize},
        script::Builder,
        transaction, Amount, ScriptBuf, Transaction, TxIn, TxOut,
    };
    use httpmock::prelude::*;
    use serde_json::json;

    fn policy() -> Policy {
        Policy {
            max_observation_age_seconds: 60,
        }
    }

    #[tokio::test]
    async fn larger_transaction_metadata_charges_the_shared_discovery_budget() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let source = source.discovery_search_snapshot();
        let txid = Txid::from_str(&"11".repeat(32)).unwrap();
        let body = json!({
            "txid": txid,
            "status": {"confirmed": false},
            "extra": "x".repeat(super::BODY_LIMIT)
        })
        .to_string();
        assert!(body.len() > super::BODY_LIMIT);
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path(format!("/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{txid}"));
            then.status(200)
                .header("x-coincube-observation", "fresh")
                .header("x-cache", "BYPASS")
                .header("cache-control", "no-store")
                .body(&body);
        });
        let budget = source.budget.as_ref().unwrap();
        let starting_requests = budget.requests.load(Ordering::Relaxed);
        let starting_bytes = budget.bytes.load(Ordering::Relaxed);
        assert_eq!(
            source
                .transaction(ChainId::BitcoinBlake2b, txid)
                .await
                .unwrap()
                .value,
            TransactionObservation::Unconfirmed { txid }
        );
        assert_eq!(
            budget.requests.load(Ordering::Relaxed),
            starting_requests - 1
        );
        assert_eq!(
            budget.bytes.load(Ordering::Relaxed),
            starting_bytes - body.len()
        );
        let nested = source.discovery_snapshot();
        assert!(Arc::ptr_eq(nested.budget.as_ref().unwrap(), budget));
        budget.bytes.store(body.len() - 1, Ordering::Relaxed);
        assert_eq!(
            nested
                .transaction(ChainId::BitcoinBlake2b, txid)
                .await
                .unwrap_err(),
            FailureKind::CollectionLimit
        );
        mock.assert_hits(2);
    }

    fn graph_transaction(parents: &[OutPoint], tag: u8) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: parents
                .iter()
                .map(|parent| TxIn {
                    previous_output: *parent,
                    script_sig: ScriptBuf::from_bytes(vec![tag]),
                    ..TxIn::default()
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(1000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[tokio::test]
    async fn canonical_pre_fork_frontier_closes_a_synthetic_graph_on_each_chain() {
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let server = MockServer::start();
            let (source, _sender) = make_source(&server);
            let prefix = HttpObservationSource::prefix(chain).unwrap();
            let tip_hash = "aa".repeat(32);
            let frontier_hash = "bb".repeat(32);
            let missing = graph_transaction(&[OutPoint::null()], 250);
            let out = |tx: &Transaction| OutPoint::new(tx.compute_txid(), 0);
            let leaves: Vec<_> = (0..5)
                .map(|tag| graph_transaction(&[out(&missing)], tag))
                .collect();
            let mut graph = leaves.clone();
            let mut previous = out(&leaves[0]);
            for tag in 10..26 {
                let tx = graph_transaction(&[previous, out(&leaves[(tag as usize) % 5])], tag);
                previous = out(&tx);
                graph.push(tx);
            }
            let selected = graph_transaction(&[previous, out(&leaves[4])], 30);
            graph.push(selected.clone());
            assert_eq!(graph.len(), 22);
            server.mock(|when, then| {
                when.method(GET)
                    .path("/api/v1/connect/networks/bitcoin-blake2b/anchor")
                    .header("authorization", "Bearer synthetic-observation-token");
                then.status(200).json_body(json!({"success":true,"data":{
                    "network":"bitcoin-blake2b","state":"available","anchor":{
                        "tip_hash":tip_hash,"tip_height":961700,
                        "tip_median_time_past":10000,"observed_at":source.now(),
                        "observation":{"tip_height":961700,"fork":{"height":961640,"active":true},
                            "rdts":{"state":"unsupported"}}
                    }
                }}));
            });
            let fresh = |path: String, body: String| {
                server.mock(|when, then| {
                    when.method(GET).path(path);
                    then.status(200)
                        .header("x-cache", "BYPASS")
                        .header("cache-control", "no-store")
                        .header("x-coincube-observation", "fresh")
                        .body(body);
                })
            };
            let base = format!("/api/v1/esplora/{prefix}");
            fresh(format!("{base}/blocks/tip/hash"), tip_hash.clone());
            fresh(
                format!("{base}/block/{tip_hash}/status"),
                json!({"in_best_chain":true,"height":961700}).to_string(),
            );
            fresh(format!("{base}/block-height/961700"), tip_hash.clone());
            fresh(format!("{base}/block-height/961600"), frontier_hash.clone());
            for (index, tx) in graph.iter().enumerate() {
                let txid = tx.compute_txid();
                server.mock(|when, then| {
                    when.method(GET).path(format!("{base}/tx/{txid}/hex"));
                    then.status(200).body(hex::encode(serialize(tx)));
                });
                let pre_fork = index < leaves.len();
                fresh(
                    format!("{base}/tx/{txid}"),
                    json!({"txid":txid,"status":{"confirmed":true,
                        "block_height":if pre_fork {961600} else {961650},
                        "block_hash":if pre_fork {&frontier_hash} else {&tip_hash}}})
                    .to_string(),
                );
            }
            let below_frontier = server.mock(|when, then| {
                when.method(GET)
                    .path(format!("{base}/tx/{}/hex", missing.compute_txid()));
                then.status(500);
            });
            let result = match chain {
                ChainId::Bitcoin => source.discover_ancestry(out(&selected), policy()).await,
                ChainId::BitcoinBlake2b => {
                    source
                        .discover_fork_ancestry(out(&selected), policy())
                        .await
                }
                _ => unreachable!(),
            };
            assert!(matches!(result, Ok(None)), "{:?}", result);
            below_frontier.assert_hits(0);
        }
    }

    #[tokio::test]
    async fn large_transaction_metadata_pre_fork_sibling_does_not_hide_post_fork_root() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let (_, raw) = ancestry_fixture();
        let mut bitcoin: Transaction = deserialize(&raw).unwrap();
        bitcoin.input[0].script_sig = Builder::new().push_int(961640).push_int(1).into_script();
        let mut fork = bitcoin.clone();
        fork.output[0].value = Amount::from_sat(4000);
        let out = |tx: &Transaction| OutPoint::new(tx.compute_txid(), 0);
        let missing = graph_transaction(&[OutPoint::null()], 250);
        let mut pre_fork_parents = vec![out(&missing)];
        pre_fork_parents.extend(
            (1..1_000).map(|n| OutPoint::new(Txid::from_str(&format!("{n:064x}")).unwrap(), 0)),
        );
        let pre_fork = graph_transaction(&pre_fork_parents, 1);
        assert!(serialize(&pre_fork).len() < claim_ancestry::MAX_TRANSACTION_BYTES);
        let large_pre_fork_status = json!({
            "txid": pre_fork.compute_txid(),
            "status": {"confirmed": true, "block_height": 961600, "block_hash": "55".repeat(32)},
            "vin": pre_fork.input.iter().map(|input| json!({
                "txid": input.previous_output.txid,
                "vout": input.previous_output.vout,
                "prevout": {
                    "scriptpubkey": "0014".to_owned() + &"66".repeat(20),
                    "scriptpubkey_asm": "OP_0 OP_PUSHBYTES_20 ".to_owned() + &"66".repeat(20),
                    "scriptpubkey_type": "v0_p2wpkh",
                    "value": 2_000
                },
                "scriptsig": "01",
                "scriptsig_asm": "OP_PUSHBYTES_1 01",
                "witness": [],
                "is_coinbase": false,
                "sequence": input.sequence.to_consensus_u32()
            })).collect::<Vec<_>>()
        })
        .to_string();
        assert!(large_pre_fork_status.len() > super::BODY_LIMIT);
        assert!(large_pre_fork_status.len() < super::TRANSACTION_METADATA_LIMIT);
        let selected = graph_transaction(&[out(&pre_fork), out(&fork)], 2);
        let bitcoin_tip = "11".repeat(32);
        let fork_tip = "22".repeat(32);
        let bitcoin_block = "33".repeat(32);
        let fork_block = "44".repeat(32);
        let frontier_block = "55".repeat(32);
        let fresh = |path: String, body: String| {
            server.mock(|when, then| {
                when.method(GET).path(path);
                then.status(200)
                    .header("x-cache", "BYPASS")
                    .header("cache-control", "no-store")
                    .header("x-coincube-observation", "fresh")
                    .body(body);
            })
        };
        for (chain, tip, block, coinbase) in [
            (ChainId::Bitcoin, &bitcoin_tip, &bitcoin_block, &bitcoin),
            (ChainId::BitcoinBlake2b, &fork_tip, &fork_block, &fork),
        ] {
            let base = format!(
                "/api/v1/esplora/{}",
                HttpObservationSource::prefix(chain).unwrap()
            );
            fresh(format!("{base}/blocks/tip/hash"), tip.clone());
            fresh(
                format!("{base}/block/{tip}/status"),
                json!({"in_best_chain":true,"height":961840}).to_string(),
            );
            fresh(format!("{base}/block-height/961840"), tip.clone());
            fresh(format!("{base}/block-height/961640"), block.clone());
            fresh(
                format!("{base}/block-height/227931"),
                "000000000000024b89b42a942fe0d9fea3bb44ab7bd1b19115dd6a759c0808b8".to_owned(),
            );
            fresh(
                format!("{base}/block/{block}/txid/0"),
                coinbase.compute_txid().to_string(),
            );
            server.mock(|when, then| {
                when.method(GET)
                    .path(format!("{base}/tx/{}/hex", coinbase.compute_txid()));
                then.status(200).body(hex::encode(serialize(coinbase)));
            });
            fresh(
                format!("{base}/tx/{}", coinbase.compute_txid()),
                json!({"txid":coinbase.compute_txid(),"status":{"confirmed":true,"block_height":961640,"block_hash":block}}).to_string(),
            );
        }
        let fork_base = "/api/v1/esplora/bitcoin-blake2b/mainnet";
        fresh(
            format!("{fork_base}/block-height/961600"),
            frontier_block.clone(),
        );
        for (tx, height, block) in [
            (&selected, 961660, &fork_tip),
            (&pre_fork, 961600, &frontier_block),
        ] {
            server.mock(|when, then| {
                when.method(GET)
                    .path(format!("{fork_base}/tx/{}/hex", tx.compute_txid()));
                then.status(200).body(hex::encode(serialize(tx)));
            });
            fresh(
                format!("{fork_base}/tx/{}", tx.compute_txid()),
                if tx.compute_txid() == pre_fork.compute_txid() {
                    large_pre_fork_status.clone()
                } else {
                    json!({"txid":tx.compute_txid(),"status":{"confirmed":true,"block_height":height,"block_hash":block}}).to_string()
                },
            );
        }
        server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/networks/bitcoin-blake2b/anchor")
                .header("authorization", "Bearer synthetic-observation-token");
            then.status(200).json_body(json!({"success":true,"data":{
                "network":"bitcoin-blake2b","state":"available","anchor":{
                    "tip_hash":fork_tip,"tip_height":961840,"tip_median_time_past":10000,
                    "observed_at":source.now(),
                    "observation":{"tip_height":961840,"fork":{"height":961640,"active":true},
                        "rdts":{"state":"unsupported"}}
                }
            }}));
        });
        let below_frontier = server.mock(|when, then| {
            when.method(GET)
                .path(format!("{fork_base}/tx/{}/hex", missing.compute_txid()));
            then.status(500);
        });
        let result = source
            .discover_fork_ancestry(out(&selected), policy())
            .await;
        assert!(matches!(result, Ok(Some(_))), "{:?}", result);
        let found = result.unwrap().unwrap();
        assert_eq!(found.links().len(), 2);
        assert_eq!(found.pair().root_chain(), ChainId::BitcoinBlake2b);
        below_frontier.assert_hits(0);
    }

    #[tokio::test]
    async fn fork_discovery_reads_only_the_fork_graph_and_fails_closed_on_observation_error() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let (txid, raw) = ancestry_fixture();
        let fork_raw = server.mock(|when, then| {
            when.method(GET).path(format!(
                "/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{txid}/hex"
            ));
            then.status(200).body(hex::encode(&raw));
        });
        let fork_status = server.mock(|when, then| {
            when.method(GET)
                .path(format!("/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{txid}"));
            then.status(503);
        });
        let bitcoin_read = server.mock(|when, then| {
            when.method(GET)
                .path_contains("/api/v1/esplora/bitcoin/mainnet/tx/");
            then.status(500);
        });
        assert!(matches!(
            source
                .discover_fork_ancestry(OutPoint::new(txid, 0), policy())
                .await,
            Err(DiscoveryError::Observation(FailureKind::Http(503)))
        ));
        fork_raw.assert_hits(1);
        fork_status.assert_hits(1);
        bitcoin_read.assert_hits(0);
    }

    #[tokio::test]
    async fn concurrent_node_reads_preserve_raw_error_precedence_and_shared_request_limit() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let (txid, raw) = ancestry_fixture();
        let base = "/api/v1/esplora/bitcoin/mainnet";
        let raw_failure = server.mock(|when, then| {
            when.method(GET).path(format!("{base}/tx/{txid}/hex"));
            then.status(503);
        });
        let malformed_metadata = server.mock(|when, then| {
            when.method(GET).path(format!("{base}/tx/{txid}"));
            then.status(200)
                .header("x-cache", "BYPASS")
                .header("cache-control", "no-store")
                .body("not JSON");
        });
        assert!(matches!(
            source
                .discover_ancestry(OutPoint::new(txid, 0), policy())
                .await,
            Err(DiscoveryError::Observation(FailureKind::Http(503)))
        ));
        raw_failure.assert_hits(1);
        malformed_metadata.assert_hits(1);

        let server = MockServer::start();
        let (mut source, _sender) = make_source(&server);
        let raw_read = server.mock(|when, then| {
            when.method(GET).path(format!("{base}/tx/{txid}/hex"));
            then.status(200).body(hex::encode(raw));
        });
        let metadata_read = server.mock(|when, then| {
            when.method(GET).path(format!("{base}/tx/{txid}"));
            then.status(200)
                .header("x-cache", "BYPASS")
                .header("cache-control", "no-store")
                .json_body(json!({"txid":txid,"status":{"confirmed":false}}));
        });
        let budget = Arc::new(CollectionBudget {
            requests: AtomicUsize::new(1),
            bytes: AtomicUsize::new(10_000),
        });
        source.budget = Some(budget.clone());
        assert!(matches!(
            source
                .discover_ancestry(OutPoint::new(txid, 0), policy())
                .await,
            Err(DiscoveryError::Observation(FailureKind::CollectionLimit))
        ));
        assert_eq!(budget.requests.load(Ordering::Relaxed), 0);
        assert_eq!(raw_read.hits() + metadata_read.hits(), 1);
    }

    #[tokio::test]
    async fn nested_snapshots_share_budget_and_stop_before_an_extra_request() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let (txid, raw) = ancestry_fixture();
        let mock = server.mock(|when, then| {
            when.method(GET);
            then.status(200).body(hex::encode(&raw));
        });
        let mut limited = source.discovery_snapshot();
        limited.budget = Some(Arc::new(CollectionBudget {
            requests: AtomicUsize::new(1),
            bytes: AtomicUsize::new(1000),
        }));
        let nested = limited.discovery_snapshot();
        assert_eq!(
            limited
                .ancestry_transaction(ChainId::Bitcoin, txid)
                .await
                .unwrap(),
            raw
        );
        assert_eq!(
            nested.ancestry_transaction(ChainId::Bitcoin, txid).await,
            Err(FailureKind::CollectionLimit)
        );
        assert!(matches!(
            nested
                .discover_ancestry(OutPoint::new(txid, 0), policy())
                .await,
            Err(DiscoveryError::Observation(FailureKind::CollectionLimit))
        ));
        let retained = claim_ancestry::retained::RetainedPath::new(
            OutPoint::new(txid, 0),
            vec![OwnedLink {
                transaction: raw.clone(),
                parent_input: None,
            }],
        )
        .unwrap();
        assert!(matches!(
            nested.requalify_ancestry(&retained, policy()).await,
            Err(DiscoveryError::Observation(FailureKind::CollectionLimit))
        ));
        mock.assert_hits(1);
        assert_eq!(
            source
                .ancestry_transaction(ChainId::Bitcoin, txid)
                .await
                .unwrap(),
            raw
        );
        mock.assert_hits(2);
    }
    #[tokio::test]
    async fn response_budget_and_remaining_graph_limit_are_enforced() {
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let (txid, raw) = ancestry_fixture();
        let mock = server.mock(|when, then| {
            when.method(GET);
            then.status(200).body(hex::encode(&raw));
        });
        let mut limited = source.discovery_snapshot();
        limited.budget = Some(Arc::new(CollectionBudget {
            requests: AtomicUsize::new(10),
            bytes: AtomicUsize::new(raw.len() * 2 - 1),
        }));
        assert_eq!(
            limited.ancestry_transaction(ChainId::Bitcoin, txid).await,
            Err(FailureKind::CollectionLimit)
        );
        assert!(matches!(
            limited
                .discover_ancestry(OutPoint::new(txid, 0), policy())
                .await,
            Err(DiscoveryError::Observation(FailureKind::CollectionLimit))
        ));
        assert_eq!(
            source
                .ancestry_transaction_limited(ChainId::Bitcoin, txid, raw.len() - 1)
                .await,
            Err(FailureKind::Malformed)
        );
        // Discovery starts both bounded node reads, even when the raw side
        // exceeds the remaining graph byte allowance.
        mock.assert_hits(4);
        let server = MockServer::start();
        let (source, _sender) = make_source(&server);
        let mock = server.mock(|when, then| {
            when.method(GET);
            then.status(500);
        });
        let mut limited = source.discovery_snapshot();
        limited.budget = Some(Arc::new(CollectionBudget {
            requests: AtomicUsize::new(10),
            bytes: AtomicUsize::new(
                crate::services::coincube::network_anchor::MAX_ANCHOR_BODY_BYTES - 1,
            ),
        }));
        assert!(matches!(
            limited.anchor(ChainId::BitcoinBlake2b).await,
            Err(FailureKind::CollectionLimit)
        ));
        mock.assert_hits(0);
    }

    #[tokio::test]
    async fn streamed_oversize_body_charges_shared_bytes_before_local_graph_limit() {
        let server = MockServer::start();
        let (mut source, _sender) = make_source(&server);
        let (txid, raw) = ancestry_fixture();
        let body = hex::encode(&raw);
        let response = server.mock(|when, then| {
            when.method(GET)
                .path(format!("/api/v1/esplora/bitcoin/mainnet/tx/{txid}/hex"));
            then.status(200)
                .header("transfer-encoding", "chunked")
                .body(body.clone());
        });
        source.budget = Some(Arc::new(CollectionBudget {
            requests: AtomicUsize::new(2),
            bytes: AtomicUsize::new(body.len() - 1),
        }));
        assert_eq!(
            source
                .ancestry_transaction_search(ChainId::Bitcoin, txid, raw.len() - 1)
                .await,
            Err((FailureKind::CollectionLimit, false))
        );
        response.assert_hits(1);
    }
    #[tokio::test]
    async fn failed_root_observation_never_becomes_an_empty_search_or_skipped_branch() {
        use coincube_core::miniscript::bitcoin::{
            consensus::{deserialize, serialize},
            script::Builder,
            Transaction, TxIn,
        };
        use serde_json::json;
        for (status, confirmed, expected) in [
            (503, false, FailureKind::Http(503)),
            (404, false, FailureKind::Changed),
            (200, false, FailureKind::Changed),
        ] {
            let server = MockServer::start();
            let (source, _sender) = make_source(&server);
            let (_, raw) = ancestry_fixture();
            let mut root: Transaction = deserialize(&raw).unwrap();
            root.input[0].script_sig = Builder::new().push_int(17).push_int(1).into_script();
            let mut later = root.clone();
            later.input[0].script_sig = Builder::new().push_int(18).push_int(1).into_script();
            let mut selected = root.clone();
            selected.input = vec![
                TxIn {
                    previous_output: OutPoint::new(root.compute_txid(), 0),
                    ..Default::default()
                },
                TxIn {
                    previous_output: OutPoint::new(later.compute_txid(), 0),
                    ..Default::default()
                },
            ];
            for tx in [&selected, &root] {
                server.mock(|when, then| {
                    when.method(GET).path(format!(
                        "/api/v1/esplora/bitcoin/mainnet/tx/{}/hex",
                        tx.compute_txid()
                    ));
                    then.status(200).body(hex::encode(serialize(tx)));
                });
            }
            server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/tx/{}",
                    selected.compute_txid()
                ));
                then.status(200)
                    .header("x-cache", "BYPASS")
                    .header("cache-control", "no-store")
                    .header("x-coincube-observation", "fresh")
                    .json_body(json!({"txid":selected.compute_txid(),"status":{"confirmed":true,"block_height":961650,"block_hash":"88".repeat(32)}}));
            });
            let later_read = server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/tx/{}/hex",
                    later.compute_txid()
                ));
                then.status(200).body(hex::encode(serialize(&later)));
            });
            server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/tx/{}",
                    root.compute_txid()
                ));
                then.status(status)
                    .header("x-cache", "BYPASS")
                    .header("cache-control", "no-store")
                    .header("x-coincube-observation", "fresh")
                    .json_body(
                        json!({"txid":root.compute_txid(),"status": if confirmed { json!({"confirmed":true,"block_height":17,"block_hash":"88".repeat(32)}) } else { json!({"confirmed":false}) }}),
                    );
            });
            server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/tx/{}", later.compute_txid()
                ));
                then.status(200)
                    .header("x-cache", "BYPASS")
                    .header("cache-control", "no-store")
                    .header("x-coincube-observation", "fresh")
                    .json_body(json!({"txid":later.compute_txid(),"status":{"confirmed":true,"block_height":18,"block_hash":"88".repeat(32)}}));
            });
            let result = source
                .discover_ancestry(
                    OutPoint::new(selected.compute_txid(), 0),
                    Policy {
                        max_observation_age_seconds: 60,
                    },
                )
                .await;
            assert!(
                matches!(result,Err(DiscoveryError::Observation(error)) if error == expected),
                "{:?}",
                result
            );
            later_read.assert_hits(0);
        }
    }
}
