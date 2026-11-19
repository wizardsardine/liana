//! Anonymous fresh Esplora reads and a separate authenticated anchor context.
use super::*;
use crate::services::coincube::{network_anchor::AnchorStartupError, CoincubeClient};
use serde::Deserialize;
use std::{
    convert::TryFrom,
    future::Future,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

mod ancestry;
mod discovery;
pub use ancestry::{CanonicalCoinbase, CoinbasePair};
pub use discovery::diagnostics::Report as AncestryTimingReport;
pub use discovery::{DiscoveredAncestry, DiscoveryError};

const BODY_LIMIT: usize = 256 * 1024;
// Esplora /tx includes every input and prevout, so its JSON can be much larger
// than the authenticated raw transaction. Keep this route separately bounded.
const TRANSACTION_METADATA_LIMIT: usize = 2 * 1024 * 1024;
const TRANSACTION_HEX_LIMIT: usize = 2 * coincube_core::claim_ancestry::MAX_TRANSACTION_BYTES;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
// A raw block can approach 4 MB; at 5 s that would need about 6.4 Mbit/s.
const RAW_BLOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// Immutable snapshot. Recreate on account/provider change; revoke its generation
/// before dropping old consumers. No inherited authenticated client headers are
/// copied into the anonymous transport, and redirects never follow transaction URLs.
pub struct HttpObservationSource {
    authenticated: CoincubeClient,
    anonymous: reqwest::Client,
    base: String,
    generation: watch::Receiver<u64>,
    expected: u64,
    budget: Option<std::sync::Arc<discovery::CollectionBudget>>,
    ancestry_trace: Option<std::sync::Arc<discovery::diagnostics::Trace>>,
    ancestry_timing_observer: Option<discovery::diagnostics::Observer>,
    #[cfg(all(test, feature = "regtest-harness"))]
    test_ancestry_history: Option<crate::services::coincube::client::TestAncestryHistory>,
}

impl HttpObservationSource {
    /// Construct a source bound to the supplied provider and session context
    /// for the supported Bitcoin/BTCB2 pair. Validate the provider URL and
    /// require a configured client token, while keeping anonymous Esplora reads
    /// on a separate transport without inherited credentials. Construction
    /// performs no network read and establishes no current verification.
    pub fn new(
        client: CoincubeClient,
        bitcoin: ChainId,
        fork: ChainId,
        context: CollectionContext,
    ) -> Result<Self, FailureKind> {
        // API has no Bitcoin testnet4 route. Legacy testnet is not interchangeable.
        if (bitcoin, fork) != (ChainId::Bitcoin, ChainId::BitcoinBlake2b) {
            return Err(FailureKind::WrongChain);
        }
        let url = reqwest::Url::parse(&client.base_url).map_err(|_| FailureKind::Malformed)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(FailureKind::Malformed);
        }
        if client.token().is_none_or(str::is_empty) {
            return Err(FailureKind::Http(401));
        }
        let anonymous = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| FailureKind::Unavailable)?;
        #[cfg(all(test, feature = "regtest-harness"))]
        let test_ancestry_history = client.test_ancestry_history();
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_owned(),
            authenticated: client,
            anonymous,
            expected: context.expected_generation,
            generation: context.generation,
            budget: None,
            ancestry_trace: None,
            ancestry_timing_observer: None,
            #[cfg(all(test, feature = "regtest-harness"))]
            test_ancestry_history,
        })
    }

    fn prefix(chain: ChainId) -> Result<&'static str, FailureKind> {
        match chain {
            ChainId::Bitcoin => Ok("bitcoin/mainnet"),
            ChainId::BitcoinBlake2b => Ok("bitcoin-blake2b/mainnet"),
            _ => Err(FailureKind::WrongChain),
        }
    }
    async fn bounded<T>(
        &self,
        future: impl Future<Output = Result<T, FailureKind>>,
    ) -> Result<T, FailureKind> {
        self.bounded_within(REQUEST_TIMEOUT, future).await
    }
    async fn bounded_within<T>(
        &self,
        deadline: Duration,
        future: impl Future<Output = Result<T, FailureKind>>,
    ) -> Result<T, FailureKind> {
        let mut generation = self.generation.clone();
        if !session_current(&generation, self.expected) {
            return Err(FailureKind::Cancelled);
        }
        let cancelled = async {
            let _ = generation.changed().await;
        };
        let result = tokio::select! { biased;
            _ = cancelled => Err(FailureKind::Cancelled),
            result = tokio::time::timeout(deadline, future) => result.map_err(|_| FailureKind::Deadline)?,
        };
        if !session_current(&generation, self.expected) {
            return Err(FailureKind::Cancelled);
        }
        result
    }
    async fn get(
        &self,
        chain: ChainId,
        path: &str,
    ) -> Result<(u16, Vec<u8>, HeaderMap, i64), FailureKind> {
        self.get_body(chain, path, true, BODY_LIMIT).await
    }
    async fn get_body(
        &self,
        chain: ChainId,
        path: &str,
        fresh: bool,
        body_limit: usize,
    ) -> Result<(u16, Vec<u8>, HeaderMap, i64), FailureKind> {
        self.get_body_detailed(chain, path, fresh, body_limit)
            .await
            .map_err(|(error, _)| error)
    }
    /// Distinguish exceeding the caller's body cap from malformed content.
    async fn get_body_detailed(
        &self,
        chain: ChainId,
        path: &str,
        fresh: bool,
        body_limit: usize,
    ) -> Result<(u16, Vec<u8>, HeaderMap, i64), (FailureKind, bool)> {
        self.get_body_within(chain, path, fresh, body_limit, REQUEST_TIMEOUT)
            .await
    }
    /// [`Self::get_body_detailed`] with its own deadline for the whole
    /// request, body included.
    async fn get_body_within(
        &self,
        chain: ChainId,
        path: &str,
        fresh: bool,
        body_limit: usize,
        deadline: Duration,
    ) -> Result<(u16, Vec<u8>, HeaderMap, i64), (FailureKind, bool)> {
        let prefix = Self::prefix(chain).map_err(|error| (error, false))?;
        let request_stage = observation_request_stage(path);
        self.bounded_within(deadline, async {
            if crate::services::esplora_backoff::remaining(&self.base).is_some() {
                return Err(FailureKind::Http(429));
            }
            let stamp = self.now();
            if let Some(budget) = &self.budget {
                budget.charge_request(0)?;
            }
            if let Some(trace) = &self.ancestry_trace {
                trace.request(chain, 0);
            }
            let mut request = self
                .anonymous
                .get(format!("{}/api/v1/esplora/{}/{}", self.base, prefix, path))
                .header(CACHE_CONTROL, "no-cache")
                .timeout(deadline);
            if fresh {
                request = request.header("X-Coincube-Observation", "fresh");
            }
            let response = request.send().await.map_err(|error| {
                if error.is_timeout() {
                    FailureKind::Deadline
                } else {
                    FailureKind::Unavailable
                }
            })?;
            self.read_body_response(response, chain, stamp, fresh, body_limit)
                .await
        })
        .await
        .map_err(|error| {
            if error == FailureKind::Deadline {
                if let Some(trace) = &self.ancestry_trace {
                    trace.emit_with_stage(
                        chain,
                        discovery::diagnostics::Outcome::RequestDeadline5s,
                        Some(request_stage),
                    );
                }
            }
            (error, false)
        })?
        .map_err(|()| (FailureKind::Malformed, true))
    }
    async fn read_body_response(
        &self,
        mut response: reqwest::Response,
        chain: ChainId,
        stamp: i64,
        fresh: bool,
        body_limit: usize,
    ) -> Result<Result<(u16, Vec<u8>, HeaderMap, i64), ()>, FailureKind> {
        let status = response.status().as_u16();
        if status == 429 {
            crate::services::esplora_backoff::record(&self.base, response.headers());
        }
        if status != 200 && status != 404 {
            return Err(FailureKind::Http(status));
        }
        let headers = response.headers().clone();
        if fresh {
            let mut markers = headers.get_all("x-coincube-observation").iter();
            if markers.next().and_then(|v| v.to_str().ok()) != Some("fresh")
                || markers.next().is_some()
            {
                return Err(FailureKind::FreshnessUnverified);
            }
            FreshRead::from_response(chain, (), stamp, &headers)?;
        }
        let Some(bytes) = self
            .read_response_bytes(&mut response, chain, body_limit)
            .await?
        else {
            return Ok(Err(()));
        };
        Ok(Ok((status, bytes, headers, stamp)))
    }

    async fn read_response_bytes(
        &self,
        response: &mut reqwest::Response,
        chain: ChainId,
        body_limit: usize,
    ) -> Result<Option<Vec<u8>>, FailureKind> {
        if response
            .content_length()
            .is_some_and(|n| n > body_limit as u64)
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                FailureKind::Deadline
            } else {
                FailureKind::Unavailable
            }
        })? {
            if let Some(budget) = &self.budget {
                budget.charge_bytes(chunk.len())?;
            }
            if let Some(trace) = &self.ancestry_trace {
                trace.bytes(chain, chunk.len());
            }
            if chunk.len() > body_limit.saturating_sub(bytes.len()) {
                return Ok(None);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    }

    /// Fetch one bounded ancestry link using the anonymous immutable-data route.
    /// Txid authentication permits cache hits, but proves neither canonical
    /// inclusion nor witness validity. These bytes must never become FreshRead
    /// evidence; the complete path and current chain views need separate checks.
    pub async fn ancestry_transaction(
        &self,
        chain: ChainId,
        txid: Txid,
    ) -> Result<Vec<u8>, FailureKind> {
        self.ancestry_transaction_limited(
            chain,
            txid,
            coincube_core::claim_ancestry::MAX_TRANSACTION_BYTES,
        )
        .await
    }
    async fn ancestry_transaction_limited(
        &self,
        chain: ChainId,
        txid: Txid,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FailureKind> {
        self.ancestry_transaction_search(chain, txid, max_bytes)
            .await
            .map_err(|(error, _)| error)
    }
    async fn ancestry_transaction_search(
        &self,
        chain: ChainId,
        txid: Txid,
        max_bytes: usize,
    ) -> Result<Vec<u8>, (FailureKind, bool)> {
        let (status, bytes, _, _) = self
            .get_body_detailed(
                chain,
                &format!("tx/{}/hex", txid),
                false,
                TRANSACTION_HEX_LIMIT.min(max_bytes.saturating_mul(2)),
            )
            .await?;
        if status != 200 {
            return Err((FailureKind::Http(status), false));
        }
        // Strict hex without whitespace keeps the wire and decoded bounds exact.
        let raw = hex::decode(bytes).map_err(|_| (FailureKind::Malformed, false))?;
        let transaction: coincube_core::miniscript::bitcoin::Transaction =
            coincube_core::miniscript::bitcoin::consensus::deserialize(&raw)
                .map_err(|_| (FailureKind::Malformed, false))?;
        if transaction.compute_txid() != txid {
            return Err((FailureKind::Malformed, false));
        }
        Ok(raw)
    }
    /// Fetch one raw block, on the same anonymous immutable-data route as
    /// [`Self::ancestry_transaction`]. Nothing here authenticates the bytes:
    /// the caller checks them against a block hash it read fresh
    /// (`coincube_core::btcb2_block::confirmed_transaction`).
    pub async fn raw_block(&self, chain: ChainId, hash: BlockHash) -> Result<Vec<u8>, FailureKind> {
        let (status, bytes, _, _) = self
            .get_body_within(
                chain,
                &format!("block/{}/raw", hash),
                false,
                coincube_core::btcb2_block::MAX_BLOCK_BYTES,
                RAW_BLOCK_TIMEOUT,
            )
            .await
            .map_err(|(error, _)| error)?;
        if status != 200 {
            return Err(FailureKind::Http(status));
        }
        Ok(bytes)
    }
    /// Six-block fee estimate from the paired fork provider, without account
    /// headers or Bitcoin-provider fallback. A quote never authorizes a spend.
    pub async fn claim_fee_rate(&self) -> Result<u64, FailureKind> {
        let (status, bytes, _, _) = self.get(ChainId::BitcoinBlake2b, "fee-estimates").await?;
        if status != 200 {
            return Err(FailureKind::Http(status));
        }
        let quotes: std::collections::BTreeMap<String, f64> =
            serde_json::from_slice(&bytes).map_err(|_| FailureKind::Malformed)?;
        let rate = quotes
            .get("6")
            .copied()
            .ok_or(FailureKind::Malformed)?
            .ceil();
        if !rate.is_finite() || rate < 1.0 || rate >= u64::MAX as f64 {
            return Err(FailureKind::Malformed);
        }
        Ok(rate as u64)
    }
    async fn hash(&self, chain: ChainId, path: &str) -> Result<FreshRead<BlockHash>, FailureKind> {
        let (status, bytes, headers, stamp) = self.get(chain, path).await?;
        if status != 200 {
            return Err(FailureKind::Http(status));
        }
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| FailureKind::Malformed)?
            .trim();
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(FailureKind::Malformed);
        }
        FreshRead::from_response(
            chain,
            BlockHash::from_str(value).map_err(|_| FailureKind::Malformed)?,
            stamp,
            &headers,
        )
    }
}

fn observation_request_stage(path: &str) -> discovery::diagnostics::Stage {
    if path.starts_with("tx/") && path.ends_with("/hex") {
        discovery::diagnostics::Stage::RawTransaction
    } else if path.starts_with("tx/") && !path.contains("/outspend/") {
        discovery::diagnostics::Stage::FreshTransaction
    } else {
        discovery::diagnostics::Stage::ChainView
    }
}

#[derive(Deserialize)]
struct BlockStatus {
    in_best_chain: bool,
    height: Option<u32>,
}
#[derive(Deserialize)]
struct TxStatus {
    confirmed: bool,
    block_height: Option<u32>,
    block_hash: Option<BlockHash>,
}
#[derive(Deserialize)]
struct TransactionInfo {
    txid: Txid,
    status: TxStatus,
}
#[async_trait]
impl ObservationSource for HttpObservationSource {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|v| i64::try_from(v.as_secs()).ok())
            .unwrap_or(-1)
    }
    async fn anchor(&self, chain: ChainId) -> Result<NetworkAnchorStatus, FailureKind> {
        if chain != ChainId::BitcoinBlake2b {
            return Err(FailureKind::WrongChain);
        }
        self.bounded(async {
            if let Some(budget) = &self.budget {
                budget.charge_request(
                    crate::services::coincube::network_anchor::MAX_ANCHOR_BODY_BYTES,
                )?;
            }
            if let Some(trace) = &self.ancestry_trace {
                trace.request(
                    chain,
                    crate::services::coincube::network_anchor::MAX_ANCHOR_BODY_BYTES,
                );
            }
            self.authenticated.network_anchor(chain).await.map_err(|e| {
                match AnchorStartupError::from(e) {
                    AnchorStartupError::Http(status) => FailureKind::Http(status),
                    AnchorStartupError::Transport => FailureKind::Unavailable,
                    _ => FailureKind::Malformed,
                }
            })
        })
        .await
        .inspect_err(|error| {
            if *error == FailureKind::Deadline {
                if let Some(trace) = &self.ancestry_trace {
                    trace.emit_with_stage(
                        chain,
                        discovery::diagnostics::Outcome::RequestDeadline5s,
                        Some(discovery::diagnostics::Stage::AuthenticatedAnchor),
                    );
                }
            }
        })
    }
    async fn tip(&self, chain: ChainId) -> Result<FreshRead<BlockRef>, FailureKind> {
        let before = self.hash(chain, "blocks/tip/hash").await?;
        let (status, bytes, headers, stamp) = self
            .get(chain, &format!("block/{}/status", before.value))
            .await?;
        if status != 200 {
            return Err(FailureKind::Http(status));
        }
        let block: BlockStatus =
            serde_json::from_slice(&bytes).map_err(|_| FailureKind::Malformed)?;
        if !block.in_best_chain {
            return Err(FailureKind::Changed);
        }
        let height = block.height.ok_or(FailureKind::Malformed)?;
        let at_height = self.hash_at_height(chain, u64::from(height)).await?;
        let after = self.hash(chain, "blocks/tip/hash").await?;
        if before.value != after.value || before.value != at_height.value {
            return Err(FailureKind::Changed);
        }
        FreshRead::from_response(
            chain,
            BlockRef {
                height: u64::from(height),
                hash: before.value,
            },
            stamp
                .min(before.observed_at)
                .min(after.observed_at)
                .min(at_height.observed_at),
            &headers,
        )
    }
    async fn transaction(
        &self,
        chain: ChainId,
        txid: Txid,
    ) -> Result<FreshRead<TransactionObservation>, FailureKind> {
        // /tx includes txid; /status alone cannot bind a body to the requested tx.
        let (status, bytes, headers, stamp) = self
            .get_body_detailed(
                chain,
                &format!("tx/{}", txid),
                true,
                TRANSACTION_METADATA_LIMIT,
            )
            .await
            .map_err(|(error, exceeded_limit)| {
                if exceeded_limit {
                    FailureKind::CollectionLimit
                } else {
                    error
                }
            })?;
        let value = if status == 404 {
            TransactionObservation::Absent
        } else {
            let tx: TransactionInfo =
                serde_json::from_slice(&bytes).map_err(|_| FailureKind::Malformed)?;
            if tx.txid != txid {
                return Err(FailureKind::Malformed);
            }
            match (
                tx.status.confirmed,
                tx.status.block_height,
                tx.status.block_hash,
            ) {
                (false, None, None) => TransactionObservation::Unconfirmed { txid },
                (true, Some(height), Some(hash)) => TransactionObservation::Confirmed {
                    txid,
                    block: BlockRef {
                        height: u64::from(height),
                        hash,
                    },
                },
                _ => return Err(FailureKind::Malformed),
            }
        };
        FreshRead::from_response(chain, value, stamp, &headers)
    }
    async fn hash_at_height(
        &self,
        chain: ChainId,
        height: u64,
    ) -> Result<FreshRead<BlockHash>, FailureKind> {
        let height = u32::try_from(height).map_err(|_| FailureKind::Malformed)?;
        self.hash(chain, &format!("block-height/{}", height)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::OutPoint;
    use httpmock::prelude::*;
    use serde_json::json;

    pub(super) fn source(server: &MockServer) -> (HttpObservationSource, watch::Sender<u64>) {
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-observation-token");
        let (sender, generation) = watch::channel(4);
        (
            HttpObservationSource::new(
                client,
                ChainId::Bitcoin,
                ChainId::BitcoinBlake2b,
                CollectionContext {
                    expected_generation: 4,
                    generation,
                },
            )
            .unwrap(),
            sender,
        )
    }
    #[tokio::test]
    async fn claim_fee_uses_fork_quote_and_refuses_missing_invalid_or_stale_values() {
        for (body, expected) in [
            (r#"{"6":1.1}"#, Some(2)),
            (r#"{"6":0}"#, None),
            (r#"{"6":-1}"#, None),
            (r#"{"1":3}"#, None),
            (r#"{"6":1e30}"#, None),
            (r#"{"6":"2"}"#, None),
        ] {
            let server = MockServer::start();
            let (source, sender) = source(&server);
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path("/api/v1/esplora/bitcoin-blake2b/mainnet/fee-estimates")
                    .header("x-coincube-observation", "fresh")
                    .matches(|request| {
                        request.headers.as_ref().is_none_or(|headers| {
                            headers.iter().all(|(name, _)| {
                                ![
                                    "authorization",
                                    "cookie",
                                    "x-device-fingerprint",
                                    "x-device-name",
                                ]
                                .iter()
                                .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
                            })
                        })
                    });
                then.status(200)
                    .header("cache-control", "no-store")
                    .header("x-cache", "BYPASS")
                    .header("x-coincube-observation", "fresh")
                    .body(body);
            });
            assert_eq!(source.claim_fee_rate().await.ok(), expected);
            mock.assert_hits(1);
            sender.send_replace(5);
            assert!(matches!(
                source.claim_fee_rate().await,
                Err(FailureKind::Cancelled)
            ));
            mock.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn raw_blocks_are_anonymous_bounded_immutable_reads() {
        let server = MockServer::start();
        let (source, _sender) = source(&server);
        let hash = BlockHash::from_str(&"ab".repeat(32)).unwrap();
        let path = format!("/api/v1/esplora/bitcoin-blake2b/mainnet/block/{}/raw", hash);
        let mut mock = server.mock(|when, then| {
            when.method(GET).path(path.clone()).matches(|request| {
                request.headers.as_ref().is_none_or(|headers| {
                    headers.iter().all(|(name, _)| {
                        !["authorization", "cookie", "x-coincube-observation"]
                            .iter()
                            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
                    })
                })
            });
            then.status(200).body([1u8, 2, 3]);
        });
        assert_eq!(
            source.raw_block(ChainId::BitcoinBlake2b, hash).await,
            Ok(vec![1, 2, 3])
        );
        mock.assert_hits(1);
        mock.delete();

        mock = server.mock(|when, then| {
            when.method(GET).path(path.clone());
            then.status(200)
                .body(vec![0u8; coincube_core::btcb2_block::MAX_BLOCK_BYTES + 1]);
        });
        assert_eq!(
            source.raw_block(ChainId::BitcoinBlake2b, hash).await,
            Err(FailureKind::Malformed)
        );
        mock.delete();

        server.mock(|when, then| {
            when.method(GET).path(path.clone());
            then.status(404);
        });
        assert_eq!(
            source.raw_block(ChainId::BitcoinBlake2b, hash).await,
            Err(FailureKind::Http(404))
        );
        assert_eq!(
            source.raw_block(ChainId::Testnet4, hash).await,
            Err(FailureKind::WrongChain)
        );
    }
    pub(super) fn ancestry_fixture() -> (Txid, Vec<u8>) {
        use coincube_core::miniscript::bitcoin::{
            absolute, consensus::serialize, transaction, Amount, Transaction, TxIn, TxOut,
        };
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn::default()],
            output: vec![TxOut {
                value: Amount::from_sat(5000),
                script_pubkey: Default::default(),
            }],
        };
        (tx.compute_txid(), serialize(&tx))
    }
    #[tokio::test]
    async fn ancestry_bytes_are_anonymous_cached_and_txid_authenticated() {
        let (txid, raw) = ancestry_fixture();
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let server = MockServer::start();
            let (source, sender) = source(&server);
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path(format!(
                        "/api/v1/esplora/{}/tx/{}/hex",
                        HttpObservationSource::prefix(chain).unwrap(),
                        txid
                    ))
                    .matches(|request| {
                        request.headers.as_ref().is_none_or(|headers| {
                            headers.iter().all(|(name, _)| {
                                ![
                                    "authorization",
                                    "cookie",
                                    "x-device-fingerprint",
                                    "x-device-name",
                                    "x-coincube-observation",
                                ]
                                .iter()
                                .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
                            })
                        })
                    });
                then.status(200)
                    .header("x-cache", "HIT")
                    .body(hex::encode(&raw));
            });
            assert_eq!(source.ancestry_transaction(chain, txid).await.unwrap(), raw);
            sender.send_replace(5);
            assert_eq!(
                source.ancestry_transaction(chain, txid).await,
                Err(FailureKind::Cancelled)
            );
            mock.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn ancestry_bytes_reject_bad_identity_encoding_size_and_status() {
        let (txid, raw) = ancestry_fixture();
        let mut trailing = raw.clone();
        trailing.push(0);
        for (requested, status, body) in [
            (id(), 200, hex::encode(&raw)),
            (txid, 200, hex::encode(trailing)),
            (txid, 200, "not hex".to_owned()),
            (txid, 200, format!("{}\n", hex::encode(&raw))),
            (txid, 200, "0".repeat(TRANSACTION_HEX_LIMIT + 1)),
            (txid, 404, hex::encode(&raw)),
            (txid, 503, hex::encode(&raw)),
        ] {
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            let mock = server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/tx/{}/hex",
                    requested
                ));
                then.status(status).body(body);
            });
            let expected = if status == 200 {
                FailureKind::Malformed
            } else {
                FailureKind::Http(status)
            };
            assert_eq!(
                source
                    .ancestry_transaction(ChainId::Bitcoin, requested)
                    .await,
                Err(expected)
            );
            mock.assert_hits(1);
        }
        let server = MockServer::start();
        let (source, _sender) = source(&server);
        assert_eq!(
            source.ancestry_transaction(ChainId::Testnet4, txid).await,
            Err(FailureKind::WrongChain)
        );
    }
    #[tokio::test]
    async fn ancestry_redirect_does_not_contact_another_provider() {
        let (txid, raw) = ancestry_fixture();
        let server = MockServer::start();
        let target = MockServer::start();
        let target_mock = target.mock(|when, then| {
            when.method(GET);
            then.status(200).body(hex::encode(raw));
        });
        let (source, _sender) = source(&server);
        let redirect = server.mock(|when, then| {
            when.method(GET);
            then.status(302).header("location", target.base_url());
        });
        assert_eq!(
            source
                .ancestry_transaction(ChainId::BitcoinBlake2b, txid)
                .await,
            Err(FailureKind::Http(302))
        );
        redirect.assert_hits(1);
        target_mock.assert_hits(0);
    }
    #[tokio::test]
    async fn ancestry_txid_does_not_authenticate_witness_and_size_bound_is_inclusive() {
        use coincube_core::miniscript::bitcoin::{
            consensus::{deserialize, serialize},
            ScriptBuf, Transaction,
        };
        let (txid, raw) = ancestry_fixture();
        let mut witness_tx: Transaction = deserialize(&raw).unwrap();
        witness_tx.input[0].witness.push([42]);
        assert_eq!(witness_tx.compute_txid(), txid);
        assert_ne!(serialize(&witness_tx), raw);
        let mut limit_tx = witness_tx.clone();
        limit_tx.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0; 399_900]);
        let size = serialize(&limit_tx).len();
        limit_tx.output[0].script_pubkey = ScriptBuf::from_bytes(vec![
            0;
            399_900 + coincube_core::claim_ancestry::MAX_TRANSACTION_BYTES
                - size
        ]);
        assert_eq!(
            serialize(&limit_tx).len(),
            coincube_core::claim_ancestry::MAX_TRANSACTION_BYTES
        );
        for tx in [witness_tx, limit_tx] {
            let bytes = serialize(&tx);
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            server.mock(|when, then| {
                when.method(GET);
                then.status(200).body(hex::encode(&bytes));
            });
            assert_eq!(
                source
                    .ancestry_transaction(ChainId::Bitcoin, tx.compute_txid())
                    .await
                    .unwrap(),
                bytes
            );
        }
    }
    #[tokio::test]
    async fn large_valid_transaction_metadata_reader_accepts_both_chains() {
        use coincube_core::miniscript::bitcoin::{
            absolute, consensus::serialize, transaction, Amount, ScriptBuf, Transaction, TxIn,
            TxOut,
        };

        let transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: (1..=1_000)
                .map(|n| TxIn {
                    previous_output: OutPoint::new(
                        Txid::from_str(&format!("{n:064x}")).unwrap(),
                        0,
                    ),
                    ..TxIn::default()
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let raw = serialize(&transaction);
        assert!(raw.len() < coincube_core::claim_ancestry::MAX_TRANSACTION_BYTES);
        let txid = transaction.compute_txid();
        let block_hash = BlockHash::from_str(&"22".repeat(32)).unwrap();
        let inputs: Vec<_> = transaction
            .input
            .iter()
            .map(|input| {
                json!({
                    "txid": input.previous_output.txid,
                    "vout": input.previous_output.vout,
                    "prevout": {
                        "scriptpubkey": "0014".to_owned() + &"33".repeat(20),
                        "scriptpubkey_asm": "OP_0 OP_PUSHBYTES_20 ".to_owned() + &"33".repeat(20),
                        "scriptpubkey_type": "v0_p2wpkh",
                        "value": 2_000
                    },
                    "scriptsig": "",
                    "scriptsig_asm": "",
                    "witness": [],
                    "is_coinbase": false,
                    "sequence": input.sequence.to_consensus_u32()
                })
            })
            .collect();
        let body = json!({
            "txid": txid,
            "status": {"confirmed": true, "block_height": 961_650, "block_hash": block_hash},
            "vin": inputs,
            "vout": [{"scriptpubkey": "", "value": 1_000}]
        })
        .to_string();
        assert!(body.len() > BODY_LIMIT);
        assert!(body.len() < TRANSACTION_METADATA_LIMIT);

        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            let prefix = HttpObservationSource::prefix(chain).unwrap();
            let raw_mock = server.mock(|when, then| {
                when.method(GET)
                    .path(format!("/api/v1/esplora/{prefix}/tx/{txid}/hex"));
                then.status(200).body(hex::encode(&raw));
            });
            let status_mock = server.mock(|when, then| {
                when.method(GET)
                    .path(format!("/api/v1/esplora/{prefix}/tx/{txid}"))
                    .header("x-coincube-observation", "fresh");
                then.status(200)
                    .header("x-coincube-observation", "fresh")
                    .header("x-cache", "BYPASS")
                    .header("cache-control", "no-store")
                    .body(&body);
            });
            assert_eq!(source.ancestry_transaction(chain, txid).await.unwrap(), raw);
            assert_eq!(
                source.transaction(chain, txid).await.unwrap().value,
                TransactionObservation::Confirmed {
                    txid,
                    block: BlockRef {
                        height: 961_650,
                        hash: block_hash
                    }
                }
            );
            raw_mock.assert_hits(1);
            status_mock.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn ancestry_cancellation_drops_inflight_read() {
        let server = MockServer::start();
        let (source, sender) = source(&server);
        let mock = server.mock(|when, then| {
            when.path(format!("/api/v1/esplora/bitcoin/mainnet/tx/{}/hex", id()));
            then.status(200).delay(Duration::from_secs(2));
        });
        let request = source.ancestry_transaction(ChainId::Bitcoin, id());
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("request completed before revocation: {:?}", result),
            reached_server = tokio::time::timeout(Duration::from_secs(1), async {
                while mock.hits_async().await == 0 {
                    tokio::task::yield_now().await;
                }
            }) => reached_server.expect("request must reach the server"),
        }
        sender.send(4).unwrap();
        let result = tokio::time::timeout(Duration::from_millis(500), request)
            .await
            .expect("revocation must not wait for the delayed response");
        assert_eq!(result.unwrap_err(), FailureKind::Cancelled);
    }
    fn id() -> Txid {
        Txid::from_str(&"11".repeat(32)).unwrap()
    }
    #[tokio::test]
    async fn anonymous_exact_route_and_requested_txid_are_preserved() {
        let server = MockServer::start();
        let (source, _sender) = source(&server);
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path(format!(
                    "/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{}",
                    id()
                ))
                .header("x-coincube-observation", "fresh")
                .header("cache-control", "no-cache")
                .matches(|request| {
                    request.headers.as_ref().is_none_or(|headers| {
                        headers.iter().all(|(name, _)| {
                            ![
                                "authorization",
                                "cookie",
                                "x-device-fingerprint",
                                "x-device-name",
                            ]
                            .iter()
                            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
                        })
                    })
                });
            then.status(200)
                .header("X-Coincube-Observation", "fresh")
                .header("X-Cache", "BYPASS")
                .header("Cache-Control", "no-store")
                .json_body(json!({"txid":id(),"status":{"confirmed":false}}));
        });
        let result = source
            .transaction(ChainId::BitcoinBlake2b, id())
            .await
            .unwrap();
        assert_eq!(result.chain, ChainId::BitcoinBlake2b);
        assert_eq!(
            result.value,
            TransactionObservation::Unconfirmed { txid: id() }
        );
        assert!(result.observed_at <= source.now());
        mock.assert();
    }
    #[tokio::test]
    async fn failures_and_missing_markers_never_become_absence() {
        for (status, marker, body, expected) in [
            (404, true, "".to_owned(), None),
            (
                404,
                false,
                "".to_owned(),
                Some(FailureKind::FreshnessUnverified),
            ),
            (503, true, "".to_owned(), Some(FailureKind::Http(503))),
            (
                200,
                true,
                "not-json".to_owned(),
                Some(FailureKind::Malformed),
            ),
            (
                200,
                true,
                json!({"txid":"22".repeat(32),"status":{"confirmed":false}}).to_string(),
                Some(FailureKind::Malformed),
            ),
            (
                200,
                true,
                "x".repeat(TRANSACTION_METADATA_LIMIT + 1),
                Some(FailureKind::CollectionLimit),
            ),
        ] {
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            let mock = server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{}",
                    id()
                ));
                let then = then
                    .status(status)
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .body(body);
                if marker {
                    then.header("X-Coincube-Observation", "fresh");
                }
            });
            match expected {
                Some(error) => assert_eq!(
                    source
                        .transaction(ChainId::BitcoinBlake2b, id())
                        .await
                        .unwrap_err(),
                    error
                ),
                None => assert_eq!(
                    source
                        .transaction(ChainId::BitcoinBlake2b, id())
                        .await
                        .unwrap()
                        .value,
                    TransactionObservation::Absent
                ),
            }
            mock.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn tip_is_hash_height_bound_and_rechecks_the_tip() {
        for wrong in [false, true] {
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            let hash = "33".repeat(32);
            let tip = server.mock(|when, then| {
                when.path("/api/v1/esplora/bitcoin/mainnet/blocks/tip/hash");
                then.status(200)
                    .header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .body(&hash);
            });
            let metadata = server.mock(|when, then| {
                when.path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/block/{}/status",
                    hash
                ));
                then.status(200)
                    .header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .json_body(json!({"in_best_chain":true,"height":105}));
            });
            let at_height = server.mock(|when, then| {
                when.path("/api/v1/esplora/bitcoin/mainnet/block-height/105");
                then.status(200)
                    .header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .body(if wrong { "44".repeat(32) } else { hash.clone() });
            });
            let result = source.tip(ChainId::Bitcoin).await;
            if wrong {
                assert_eq!(result.unwrap_err(), FailureKind::Changed);
            } else {
                assert_eq!(
                    result.unwrap().value,
                    BlockRef {
                        height: 105,
                        hash: BlockHash::from_str(&hash).unwrap()
                    }
                );
            }
            tip.assert_hits(2);
            metadata.assert_hits(1);
            at_height.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn unsupported_pair_revocation_and_wrong_chain_refuse_without_io() {
        let server = MockServer::start();
        let (source, sender) = source(&server);
        let any = server.mock(|_, then| {
            then.status(500);
        });
        assert_eq!(
            source.tip(ChainId::Testnet4).await.unwrap_err(),
            FailureKind::WrongChain
        );
        assert_eq!(
            source.anchor(ChainId::Bitcoin).await.unwrap_err(),
            FailureKind::WrongChain
        );
        let client = source.authenticated.clone();
        let (_, generation) = watch::channel(4);
        assert!(matches!(
            HttpObservationSource::new(
                client,
                ChainId::Testnet4,
                ChainId::BitcoinBlake2bTestnet4,
                CollectionContext {
                    expected_generation: 4,
                    generation
                }
            ),
            Err(FailureKind::WrongChain)
        ));
        sender.send(5).unwrap();
        assert_eq!(
            source
                .transaction(ChainId::Bitcoin, id())
                .await
                .unwrap_err(),
            FailureKind::Cancelled
        );
        any.assert_hits(0);
    }
    #[tokio::test]
    async fn anchor_401_and_redirects_remain_errors_without_following() {
        for status in [401, 302] {
            let server = MockServer::start();
            let (source, _sender) = source(&server);
            let anchor = server.mock(|when, then| {
                when.path("/api/v1/connect/networks/bitcoin-blake2b/anchor")
                    .header("authorization", "Bearer synthetic-observation-token");
                then.status(status)
                    .header("Location", format!("{}/must-not-follow", server.base_url()));
            });
            let redirected = server.mock(|when, then| {
                when.path("/must-not-follow");
                then.status(200);
            });
            assert_eq!(
                source.anchor(ChainId::BitcoinBlake2b).await.unwrap_err(),
                FailureKind::Http(status)
            );
            anchor.assert_hits(1);
            redirected.assert_hits(0);
        }
    }
    #[tokio::test]
    async fn anonymous_redirect_never_reaches_a_second_endpoint() {
        let server = MockServer::start();
        let (source, _sender) = source(&server);
        let redirect = server.mock(|when, then| {
            when.path(format!(
                "/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{}",
                id()
            ));
            then.status(302).header(
                "Location",
                format!("{}/bitcoin-fallback", server.base_url()),
            );
        });
        let fallback = server.mock(|when, then| {
            when.path("/bitcoin-fallback");
            then.status(200);
        });
        assert_eq!(
            source
                .transaction(ChainId::BitcoinBlake2b, id())
                .await
                .unwrap_err(),
            FailureKind::Http(302)
        );
        redirect.assert_hits(1);
        fallback.assert_hits(0);
    }

    #[tokio::test]
    async fn cancellation_drops_inflight_http_future() {
        let server = MockServer::start();
        let (source, sender) = source(&server);
        server.mock(|when, then| {
            when.path(format!("/api/v1/esplora/bitcoin/mainnet/tx/{}", id()));
            then.status(200).delay(Duration::from_secs(2));
        });
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            sender.send(5).unwrap();
        };
        let (result, ()) = tokio::join!(source.transaction(ChainId::Bitcoin, id()), revoke);
        assert_eq!(result.unwrap_err(), FailureKind::Cancelled);
    }
}
