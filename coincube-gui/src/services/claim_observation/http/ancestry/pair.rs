//! Mainnet-only root-pair collection. No spend authorization is produced.
use super::*;

// Pinned Knots v29.4.1.knots20260508 src/kernel/chainparams.cpp (mainnet)
// and src/validation.cpp BIP34_IMPLIES_BIP30_LIMIT. The known pre-BIP34
// history has no future-height prefix in this interval. Do not generalize
// this to arbitrary histories, regtest, the 490897 exception, or >=1983702.
const FIRST_FORK_HEIGHT: u32 = 961_640;
const HISTORICAL_LIMIT: u32 = 1_983_702;
const BIP34_HEIGHT: u64 = 227_931;
const BIP34_HASH: &str = "000000000000024b89b42a942fe0d9fea3bb44ab7bd1b19115dd6a759c0808b8";

#[derive(Clone, Copy)]
struct AncestryHistory {
    first_fork_height: u32,
    historical_limit: u32,
    shared_history_height: u64,
    shared_history_hash: BlockHash,
}

impl HttpObservationSource {
    fn ancestry_history(&self) -> Result<AncestryHistory, FailureKind> {
        #[cfg(all(test, feature = "regtest-harness"))]
        if let Some(history) = self.test_ancestry_history {
            return Ok(AncestryHistory {
                first_fork_height: history.first_fork_height,
                historical_limit: history.historical_limit,
                shared_history_height: history.shared_history_height,
                shared_history_hash: history.shared_history_hash,
            });
        }
        Ok(AncestryHistory {
            first_fork_height: FIRST_FORK_HEIGHT,
            historical_limit: HISTORICAL_LIMIT,
            shared_history_height: BIP34_HEIGHT,
            shared_history_hash: BlockHash::from_str(BIP34_HASH)
                .map_err(|_| FailureKind::Malformed)?,
        })
    }
    pub(in crate::services::claim_observation::http) fn ancestry_first_fork_height(
        &self,
    ) -> Result<u32, FailureKind> {
        Ok(self.ancestry_history()?.first_fork_height)
    }
}

/// Matching current mainnet histories with distinct same-height coinbase roots.
/// Still only provider-trusted observations: the selected output's ownership,
/// maturity, spendability, complete ancestry path and submission policy remain
/// separate. Recollect after restart/reorg and recheck generation before use.
#[derive(Debug)]
pub struct CoinbasePair {
    selected: OutPoint,
    root_chain: ChainId,
    bitcoin: CanonicalCoinbase,
    fork: CanonicalCoinbase,
    anchor: super::super::super::NetworkAnchor,
    observed_at: i64,
    generation: u64,
}
impl CoinbasePair {
    pub fn selected(&self) -> OutPoint {
        self.selected
    }
    pub fn root_chain(&self) -> ChainId {
        self.root_chain
    }
    pub fn bitcoin(&self) -> &CanonicalCoinbase {
        &self.bitcoin
    }
    pub fn fork(&self) -> &CanonicalCoinbase {
        &self.fork
    }
    pub fn anchor(&self) -> &super::super::super::NetworkAnchor {
        &self.anchor
    }
    pub fn observed_at(&self) -> i64 {
        self.observed_at
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl HttpObservationSource {
    /// Bind a previously verified dependency to positive observations from both
    /// mainnet chains and their known common pre-BIP34 history. No negative
    /// transaction lookup is used. The complete collection shares a 30s budget.
    pub async fn coinbase_pair(
        &self,
        dependency: &claim_ancestry::CoinbaseDependency,
        height: u32,
        policy: Policy,
    ) -> Result<CoinbasePair, FailureKind> {
        self.coinbase_pair_for(dependency, height, policy, ChainId::Bitcoin)
            .await
    }

    /// The same positive two-chain root comparison for a dependency whose
    /// canonical coinbase must be on BTCB2. This does not establish current
    /// UTXO spendability, ownership or permission to sign.
    pub async fn fork_coinbase_pair(
        &self,
        dependency: &claim_ancestry::CoinbaseDependency,
        height: u32,
        policy: Policy,
    ) -> Result<CoinbasePair, FailureKind> {
        self.coinbase_pair_for(dependency, height, policy, ChainId::BitcoinBlake2b)
            .await
    }

    async fn coinbase_pair_for(
        &self,
        dependency: &claim_ancestry::CoinbaseDependency,
        height: u32,
        policy: Policy,
        root_chain: ChainId,
    ) -> Result<CoinbasePair, FailureKind> {
        let history = self.ancestry_history()?;
        if !(history.first_fork_height..history.historical_limit).contains(&height) {
            return Err(FailureKind::UnsupportedPoison);
        }
        dependency
            .check_height_commitment(height)
            .map_err(|_| FailureKind::Malformed)?;
        tokio::time::timeout(MAX_COLLECTION_TIME, async {
            let fork_chain = ChainId::BitcoinBlake2b;
            let before = super::super::super::checked_ancestry_anchor(
                self.anchor(fork_chain).await?,
                fork_chain,
                policy,
                self.now(),
            )
            .map_err(|e| e.kind)?;
            if before
                .observation
                .fork
                .as_ref()
                .is_none_or(|fork| fork.height != u64::from(history.first_fork_height))
            {
                return Err(FailureKind::WrongChain);
            }
            let bitcoin_history = self
                .hash_at_height(ChainId::Bitcoin, history.shared_history_height)
                .await?;
            let fork_history = self
                .hash_at_height(fork_chain, history.shared_history_height)
                .await?;
            if bitcoin_history.value != history.shared_history_hash
                || fork_history.value != history.shared_history_hash
            {
                return Err(FailureKind::WrongChain);
            }
            let bitcoin = self.canonical_coinbase(ChainId::Bitcoin, height).await?;
            let fork = self.canonical_coinbase(fork_chain, height).await?;
            if fork.value.tip.hash != before.tip_hash || fork.value.tip.height != before.tip_height
            {
                return Err(FailureKind::Changed);
            }
            let mut oldest = before
                .observed_at
                .min(bitcoin_history.observed_at)
                .min(fork_history.observed_at)
                .min(bitcoin.observed_at)
                .min(fork.observed_at);
            oldest = self
                .recheck_coinbase_views(
                    [
                        (ChainId::Bitcoin, &bitcoin.value),
                        (fork_chain, &fork.value),
                    ],
                    history,
                    height,
                    oldest,
                )
                .await?;
            let after = super::super::super::checked_ancestry_anchor(
                self.anchor(fork_chain).await?,
                fork_chain,
                policy,
                self.now(),
            )
            .map_err(|e| e.kind)?;
            if before.tip_hash != after.tip_hash
                || before.tip_height != after.tip_height
                || before.tip_median_time_past != after.tip_median_time_past
                || before.observation != after.observation
            {
                return Err(FailureKind::Changed);
            }
            oldest = oldest.min(after.observed_at);
            if !super::super::super::fresh(oldest, self.now(), policy.max_observation_age_seconds) {
                return Err(FailureKind::Stale);
            }
            if !session_current(&self.generation, self.expected) {
                return Err(FailureKind::Cancelled);
            }
            // Classify invalidation only after both histories, height mappings,
            // tips and the authenticated anchor stayed unchanged across the
            // complete fresh collection. A provider error or bracket race must
            // remain indeterminate and cannot revoke durable completion.
            let selected_root = match root_chain {
                ChainId::Bitcoin => bitcoin.value.txid,
                ChainId::BitcoinBlake2b => fork.value.txid,
                _ => return Err(FailureKind::WrongChain),
            };
            if selected_root != dependency.root().txid {
                return Err(FailureKind::AncestryRootChanged {
                    observed_at: oldest,
                });
            }
            if bitcoin.value.txid == fork.value.txid {
                return Err(FailureKind::AncestryRootShared {
                    observed_at: oldest,
                });
            }
            Ok(CoinbasePair {
                selected: dependency.selected(),
                root_chain,
                bitcoin: bitcoin.value,
                fork: fork.value,
                anchor: after,
                observed_at: oldest,
                generation: self.expected,
            })
        })
        .await
        .map_err(|_| {
            if let Some(trace) = &self.ancestry_trace {
                trace.emit(
                    root_chain,
                    super::super::discovery::diagnostics::Outcome::CollectionDeadline30s,
                );
            }
            FailureKind::Deadline
        })?
    }
    async fn recheck_coinbase_views(
        &self,
        roots: [(ChainId, &CanonicalCoinbase); 2],
        history: AncestryHistory,
        height: u32,
        mut oldest: i64,
    ) -> Result<i64, FailureKind> {
        for (chain, root) in roots {
            let shared = self
                .hash_at_height(chain, history.shared_history_height)
                .await?;
            let block = self.hash_at_height(chain, u64::from(height)).await?;
            let tip = self.tip(chain).await?;
            if shared.value != history.shared_history_hash
                || block.value != root.block.hash
                || tip.value != root.tip
            {
                return Err(FailureKind::Changed);
            }
            oldest = oldest
                .min(shared.observed_at)
                .min(block.observed_at)
                .min(tip.observed_at);
        }
        Ok(oldest)
    }
}

#[cfg(all(test, feature = "regtest-harness"))]
mod tests {
    use super::super::super::tests::source;
    use super::*;
    use httpmock::MockServer;

    #[test]
    fn explicit_regtest_history_is_local_to_the_test_client() {
        let server = MockServer::start();
        let shared = BlockHash::from_str(&"09".repeat(32)).unwrap();
        let mut client = crate::services::coincube::CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-observation-token");
        client.enable_regtest_ancestry(110, 213, 109, shared);
        let (_sender, generation) = watch::channel(4);
        let configured_source = HttpObservationSource::new(
            client,
            ChainId::Bitcoin,
            ChainId::BitcoinBlake2b,
            CollectionContext {
                expected_generation: 4,
                generation,
            },
        )
        .unwrap();
        let configured = configured_source.ancestry_history().unwrap();
        assert_eq!(configured.first_fork_height, 110);
        assert_eq!(configured.historical_limit, 213);
        assert_eq!(configured.shared_history_height, 109);
        assert_eq!(configured.shared_history_hash, shared);

        let (default, _sender) = source(&server);
        let pinned = default.ancestry_history().unwrap();
        assert_eq!(pinned.first_fork_height, FIRST_FORK_HEIGHT);
        assert_eq!(pinned.historical_limit, HISTORICAL_LIMIT);
        assert_eq!(pinned.shared_history_height, BIP34_HEIGHT);
        assert_eq!(pinned.shared_history_hash.to_string(), BIP34_HASH);
    }
}
