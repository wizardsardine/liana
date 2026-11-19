//! Anonymous, fresh-only reads from Connect's Esplora proxy, shared by the
//! Split coins fee sources (`split_fees`).

use std::{convert::TryFrom, time::Duration};

use coincube_core::{
    chain::ChainId,
    miniscript::bitcoin::{OutPoint, Txid},
};
use reqwest::header::{HeaderMap, CACHE_CONTROL};
use serde::Deserialize;
use tokio::sync::watch;

use super::{
    claim_observation::{CollectionContext, FailureKind, FreshRead},
    coincube::CoincubeClient,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const JSON_LIMIT: usize = 64 * 1024;
/// An address's unspent-output list (the foreign scanner's per-response cap).
const UTXO_LIST_LIMIT: usize = 2 * 1024 * 1024;

/// Anonymous, fresh-only reads from Connect's Esplora proxy for the paths the
/// Claim observation source has no method for: an address's unspent outputs
/// and fee estimates. Only paths on Connect's fresh-observation allowlist are
/// requested (Connect answers any other fresh path with 400). No account
/// header is sent, redirects are never followed, every response must
/// acknowledge the fresh-read contract (`X-Coincube-Observation: fresh`,
/// `X-Cache: BYPASS`, `Cache-Control: no-store`), and a changed generation
/// cancels an in-flight read.
pub struct ConnectEsplora {
    anonymous: reqwest::Client,
    base: String,
    expected: u64,
    generation: watch::Receiver<u64>,
}

impl ConnectEsplora {
    pub fn new(client: &CoincubeClient, context: CollectionContext) -> Result<Self, FailureKind> {
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
        Ok(Self {
            anonymous: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|_| FailureKind::Unavailable)?,
            base: url.as_str().trim_end_matches('/').to_owned(),
            expected: context.expected_generation,
            generation: context.generation,
        })
    }

    pub(crate) fn rate_limit_retry_after(&self) -> Option<Duration> {
        crate::services::esplora_backoff::remaining(&self.base)
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
            .unwrap_or(-1)
    }

    /// One fresh GET. Only 200 is a value; 404 and every other status fail.
    pub async fn fresh(
        &self,
        chain: ChainId,
        path: &str,
        limit: usize,
    ) -> Result<FreshRead<Vec<u8>>, FailureKind> {
        let prefix = match chain {
            ChainId::Bitcoin => "bitcoin/mainnet",
            ChainId::BitcoinBlake2b => "bitcoin-blake2b/mainnet",
            _ => return Err(FailureKind::WrongChain),
        };
        let mut generation = self.generation.clone();
        if *generation.borrow() != self.expected || generation.has_changed().is_err() {
            return Err(FailureKind::Cancelled);
        }
        let cancelled = async {
            loop {
                if generation.changed().await.is_err()
                    || *generation.borrow_and_update() != self.expected
                {
                    break;
                }
            }
        };
        let read = self.read_fresh_response(chain, prefix, path, limit);
        let result = tokio::select! { biased;
            _ = cancelled => Err(FailureKind::Cancelled),
            result = tokio::time::timeout(REQUEST_TIMEOUT, read) => {
                result.map_err(|_| FailureKind::Deadline)?
            }
        };
        if *self.generation.borrow() != self.expected || self.generation.has_changed().is_err() {
            return Err(FailureKind::Cancelled);
        }
        result
    }

    async fn read_fresh_response(
        &self,
        chain: ChainId,
        prefix: &str,
        path: &str,
        limit: usize,
    ) -> Result<FreshRead<Vec<u8>>, FailureKind> {
        if self.rate_limit_retry_after().is_some() {
            return Err(FailureKind::Http(429));
        }
        let stamp = Self::now();
        let mut response = self
            .anonymous
            .get(format!("{}/api/v1/esplora/{prefix}/{path}", self.base))
            .header(CACHE_CONTROL, "no-cache")
            .header("X-Coincube-Observation", "fresh")
            .send()
            .await
            .map_err(|_| FailureKind::Unavailable)?;
        let status = response.status().as_u16();
        if status == 429 {
            crate::services::esplora_backoff::record(&self.base, response.headers());
        }
        if status != 200 {
            return Err(FailureKind::Http(status));
        }
        let headers: HeaderMap = response.headers().clone();
        let mut markers = headers.get_all("x-coincube-observation").iter();
        if markers.next().and_then(|v| v.to_str().ok()) != Some("fresh") || markers.next().is_some()
        {
            return Err(FailureKind::FreshnessUnverified);
        }
        FreshRead::from_response(chain, (), stamp, &headers)?;
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(FailureKind::Malformed);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FailureKind::Unavailable)?
        {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(FailureKind::Malformed);
            }
            bytes.extend_from_slice(&chunk);
        }
        FreshRead::from_response(chain, bytes, stamp, &headers)
    }

    /// The outpoints of an address's unspent outputs.
    pub async fn unspent_outputs(
        &self,
        chain: ChainId,
        address: &str,
    ) -> Result<FreshRead<Vec<OutPoint>>, FailureKind> {
        #[derive(Deserialize)]
        struct Utxo {
            txid: Txid,
            vout: u32,
        }
        if address.is_empty() || !address.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(FailureKind::Malformed);
        }
        let read = self
            .fresh(chain, &format!("address/{address}/utxo"), UTXO_LIST_LIMIT)
            .await?;
        let utxos: Vec<Utxo> =
            serde_json::from_slice(read.value()).map_err(|_| FailureKind::Malformed)?;
        FreshRead::from_response(
            chain,
            utxos
                .into_iter()
                .map(|utxo| OutPoint::new(utxo.txid, utxo.vout))
                .collect(),
            read.observed_at(),
            &fresh_headers(),
        )
    }

    /// Whether an address has any history on `chain`: a funded output or a
    /// transaction, confirmed or in the mempool (`address/{addr}` stats).
    /// `false` is fresh proof the address was never used there (#568 B3b,
    /// #592 I10/I11): it reads the chain through Connect, not a daemon's
    /// last poll (N1).
    pub async fn address_used(
        &self,
        chain: ChainId,
        address: &str,
    ) -> Result<FreshRead<bool>, FailureKind> {
        #[derive(Deserialize)]
        struct Stats {
            funded_txo_count: u64,
            spent_txo_count: u64,
            tx_count: u64,
        }
        #[derive(Deserialize)]
        struct AddressStats {
            address: String,
            chain_stats: Stats,
            mempool_stats: Stats,
        }
        if address.is_empty() || !address.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(FailureKind::Malformed);
        }
        let read = self
            .fresh(chain, &format!("address/{address}"), JSON_LIMIT)
            .await?;
        let stats: AddressStats =
            serde_json::from_slice(read.value()).map_err(|_| FailureKind::Malformed)?;
        if stats.address != address {
            return Err(FailureKind::Malformed);
        }
        let used = [stats.chain_stats, stats.mempool_stats]
            .iter()
            .any(|s| s.funded_txo_count > 0 || s.spent_txo_count > 0 || s.tx_count > 0);
        FreshRead::from_response(chain, used, read.observed_at(), &fresh_headers())
    }

    /// Six-block fee estimate, rounded up, in sat/vB.
    pub async fn fee_rate(&self, chain: ChainId) -> Result<u64, FailureKind> {
        let read = self.fresh(chain, "fee-estimates", JSON_LIMIT).await?;
        let quotes: std::collections::BTreeMap<String, f64> =
            serde_json::from_slice(read.value()).map_err(|_| FailureKind::Malformed)?;
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
}

/// Re-wraps a value decoded from an already acknowledged fresh response.
fn fresh_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-cache", "BYPASS".parse().expect("static header"));
    headers.insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
    headers
}
