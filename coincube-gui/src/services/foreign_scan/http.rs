use super::*;
use crate::services::coincube::network_anchor::AnchorState;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
const BODY: usize = 2 * 1024 * 1024;
const TOTAL: usize = 16 * 1024 * 1024;
pub(super) struct HttpSource {
    authenticated: CoincubeClient,
    anonymous: reqwest::Client,
    base: String,
    bytes: AtomicUsize,
}
impl HttpSource {
    pub(super) fn new(client: CoincubeClient) -> Result<Self, ScanError> {
        let url = reqwest::Url::parse(&client.base_url).map_err(|_| ScanError::Malformed)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(ScanError::Malformed);
        }
        if client.token().is_none_or(str::is_empty) {
            return Err(ScanError::Http(401));
        }
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_owned(),
            authenticated: client,
            anonymous: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| ScanError::Unavailable)?,
            bytes: AtomicUsize::new(0),
        })
    }
    async fn get(&self, chain: ChainId, path: &str, fresh: bool) -> Result<Vec<u8>, ScanError> {
        if let Some(delay) = crate::services::esplora_backoff::remaining(&self.base) {
            return Err(ScanError::RateLimited(delay));
        }
        let network = match chain {
            ChainId::Bitcoin => "bitcoin/mainnet",
            ChainId::BitcoinBlake2b => "bitcoin-blake2b/mainnet",
            _ => return Err(ScanError::UnsupportedChain),
        };
        let mut request = self
            .anonymous
            .get(format!("{}/api/v1/esplora/{}/{}", self.base, network, path))
            .header("Cache-Control", "no-cache");
        if fresh {
            request = request.header("X-Coincube-Observation", "fresh");
        }
        let mut response = request.send().await.map_err(|_| ScanError::Unavailable)?;
        if response.status().as_u16() == 429 {
            return Err(ScanError::RateLimited(
                crate::services::esplora_backoff::record(&self.base, response.headers()),
            ));
        }
        if response.status().as_u16() != 200 {
            return Err(ScanError::Http(response.status().as_u16()));
        }
        if fresh {
            validate_fresh_headers(&response)?;
        }
        if response.content_length().is_some_and(|v| v > BODY as u64) {
            return Err(ScanError::BodyLimit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ScanError::Unavailable)? {
            if chunk.len() > BODY.saturating_sub(bytes.len())
                || self
                    .bytes
                    .fetch_add(chunk.len(), Ordering::Relaxed)
                    .saturating_add(chunk.len())
                    > TOTAL
            {
                return Err(ScanError::BodyLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub(super) async fn current_height(&self, chain: ChainId) -> Result<u32, ScanError> {
        let bytes = self.get(chain, "blocks/tip/height", true).await?;
        std::str::from_utf8(&bytes)
            .map_err(|_| ScanError::Malformed)?
            .trim()
            .parse()
            .map_err(|_| ScanError::Malformed)
    }
}
#[async_trait]
impl Source for HttpSource {
    async fn tip(&self, chain: ChainId) -> Result<BlockHash, ScanError> {
        let bytes = self.get(chain, "blocks/tip/hash", true).await?;
        let hash = std::str::from_utf8(&bytes)
            .map_err(|_| ScanError::Malformed)?
            .trim();
        if hash.len() != 64 {
            return Err(ScanError::Malformed);
        }
        BlockHash::from_str(hash).map_err(|_| ScanError::Malformed)
    }
    async fn tip_height(&self, chain: ChainId, tip: BlockHash) -> Result<u32, ScanError> {
        #[derive(Deserialize)]
        struct BlockStatus {
            in_best_chain: bool,
            height: Option<u32>,
        }
        let status: BlockStatus = serde_json::from_slice(
            &self
                .get(chain, &format!("block/{tip}/status"), true)
                .await?,
        )
        .map_err(|_| ScanError::Malformed)?;
        if !status.in_best_chain {
            return Err(ScanError::Changed(ChangedReason::TipNoLongerBest));
        }
        status.height.ok_or(ScanError::Malformed)
    }
    async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
        let status = self
            .authenticated
            .network_anchor(ChainId::BitcoinBlake2b)
            .await
            .map_err(|_| ScanError::Unavailable)?;
        if status.network != ChainId::BitcoinBlake2b || status.state != AnchorState::Available {
            return Err(ScanError::Unavailable);
        }
        let anchor = status.anchor.ok_or(ScanError::Malformed)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ScanError::Malformed)?
            .as_secs();
        if anchor.observed_at < 0
            || now
                .checked_sub(anchor.observed_at as u64)
                .is_none_or(|age| age > 30)
        {
            return Err(ScanError::Freshness);
        }
        let fork_height = anchor
            .observation
            .fork
            .as_ref()
            .filter(|fork| fork.active)
            .map(|fork| fork.height);
        Ok((anchor.tip_hash, fork_height))
    }
    async fn stats(&self, chain: ChainId, address: &str) -> Result<Stats, ScanError> {
        serde_json::from_slice(
            &self
                .get(chain, &format!("address/{}", address), true)
                .await?,
        )
        .map_err(|_| ScanError::Malformed)
    }
    async fn utxos(&self, chain: ChainId, address: &str) -> Result<Vec<Utxo>, ScanError> {
        serde_json::from_slice(
            &self
                .get(chain, &format!("address/{}/utxo", address), true)
                .await?,
        )
        .map_err(|_| ScanError::Malformed)
    }
    async fn transaction(&self, chain: ChainId, txid: Txid) -> Result<Transaction, ScanError> {
        let bytes = self.get(chain, &format!("tx/{}/hex", txid), false).await?;
        let raw = hex::decode(
            std::str::from_utf8(&bytes)
                .map_err(|_| ScanError::Malformed)?
                .trim(),
        )
        .map_err(|_| ScanError::Malformed)?;
        let transaction: Transaction =
            bitcoin::consensus::deserialize(&raw).map_err(|_| ScanError::Malformed)?;
        if transaction.compute_txid() != txid {
            return Err(ScanError::Prevout);
        }
        Ok(transaction)
    }
}

fn validate_fresh_headers(response: &reqwest::Response) -> Result<(), ScanError> {
    for (name, expected) in [("x-coincube-observation", "fresh"), ("x-cache", "BYPASS")] {
        let mut values = response.headers().get_all(name).iter();
        if values.next().and_then(|v| v.to_str().ok()) != Some(expected) || values.next().is_some()
        {
            return Err(ScanError::Freshness);
        }
    }
    if !response
        .headers()
        .get_all("cache-control")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .any(|s| s.trim().eq_ignore_ascii_case("no-store"))
    {
        return Err(ScanError::Freshness);
    }

    Ok(())
}
