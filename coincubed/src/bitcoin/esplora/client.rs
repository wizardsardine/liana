use bdk_electrum::bdk_chain::{
    bitcoin,
    spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult},
};
use bdk_esplora::{esplora_client, EsploraExt};

use crate::bitcoin::BlockChainTip;

const REQUEST_TIMEOUT_SECS: u64 = 30;

/// An error from the Esplora client.
#[derive(Debug)]
pub enum Error {
    Client(Box<esplora_client::Error>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Client(e) => write!(f, "Esplora client error: '{}'.", e),
        }
    }
}

/// Bitcoin Esplora client with optional secondary endpoint used as failover.
///
/// `try_with_fallback` runs an operation against `primary` first. If the result
/// is "retryable" per [`should_fall_back`] — transport error, HTTP 402 (quota
/// exhausted on Blockstream Enterprise), 429 (rate limited), or 5xx — and a
/// `fallback` client is configured, the same operation is replayed against
/// `fallback`. All other results pass through unchanged.
///
/// Methods that consume a request (`sync`, `full_scan`) take a builder closure
/// so the request can be rebuilt for the fallback attempt — the BDK request
/// types are moved into the call and aren't trivially clonable.
pub struct Client {
    primary: esplora_client::blocking::BlockingClient,
    fallback: Option<esplora_client::blocking::BlockingClient>,
}

/// Build a `BlockingClient` for the given address, applying our standard
/// timeout, the gzip-disabling header, and an optional bearer token.
fn build_blocking_client(
    addr: &str,
    token: Option<&str>,
) -> esplora_client::blocking::BlockingClient {
    let mut builder = esplora_client::Builder::new(addr).timeout(REQUEST_TIMEOUT_SECS);
    // The blocking esplora client uses `minreq` underneath, which has no
    // content-encoding support. If the server returns gzip/brotli-compressed
    // bodies (common for /address/:addr/txs and other large responses),
    // minreq tries to read the raw bytes as UTF-8 and fails with
    // `InvalidUtf8InResponse`. Force the server to send uncompressed
    // responses to avoid this.
    builder = builder.header("Accept-Encoding", "identity");
    if let Some(token) = token {
        builder = builder.header("Authorization", &format!("Bearer {}", token));
    }
    builder.build_blocking()
}

/// Reports whether an esplora call's result should trigger a fallback retry.
///
/// HTTP 402 and 429 are 4xx codes but they describe the provider's *capacity*
/// rather than the request — falling back to a second provider is the correct
/// response. Genuine 4xx outcomes like 400/404 describe the request itself and
/// pass through unchanged so the caller sees the real answer.
fn should_fall_back<T>(result: &Result<T, esplora_client::Error>) -> bool {
    match result {
        Ok(_) => false,
        Err(esplora_client::Error::HttpResponse { status, .. }) => {
            matches!(*status, 402 | 429) || (500..=599).contains(status)
        }
        // Transport-level errors (connection refused, timeout, TLS, parse, …)
        // — anything that isn't a deliberate HTTP response from the provider.
        Err(_) => true,
    }
}

impl Client {
    /// Create a new client and verify connectivity by fetching the current tip height.
    ///
    /// Tries primary first; if primary's connectivity check fails and a
    /// fallback is configured, tries fallback. Construction only fails when
    /// neither endpoint is reachable.
    pub fn new(config: &crate::config::EsploraConfig) -> Result<Self, Error> {
        let primary = build_blocking_client(&config.addr, config.token.as_deref());
        let fallback = config.fallback_addr.as_deref().map(|addr| {
            build_blocking_client(addr, config.fallback_token.as_deref())
        });

        // Verify we can reach the server. Fallback to the secondary if the
        // primary is unreachable at startup so a rate-limited primary doesn't
        // block daemon launch.
        let primary_check = primary.get_height();
        if primary_check.is_ok() {
            return Ok(Client { primary, fallback });
        }
        if let Some(ref fb) = fallback {
            if fb.get_height().is_ok() {
                log::warn!(
                    "Esplora primary unreachable at startup; using fallback for connectivity check"
                );
                return Ok(Client { primary, fallback });
            }
        }
        Err(Error::Client(Box::new(primary_check.unwrap_err())))
    }

    /// Run `op` against the primary client. On a retryable failure, replay it
    /// against the fallback (if configured). See [`should_fall_back`].
    fn try_with_fallback<T, F>(&self, mut op: F) -> Result<T, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
    {
        let primary_result = op(&self.primary);
        if !should_fall_back(&primary_result) {
            return primary_result.map_err(|e| Error::Client(Box::new(e)));
        }
        let fallback = match &self.fallback {
            Some(fb) => fb,
            None => return primary_result.map_err(|e| Error::Client(Box::new(e))),
        };
        if let Err(ref e) = primary_result {
            log::warn!("Esplora primary failed ({}); retrying on fallback", e);
        }
        op(fallback).map_err(|e| Error::Client(Box::new(e)))
    }

    /// Get the genesis block hash (block at height 0).
    pub fn genesis_block_hash(&self) -> Result<bitcoin::BlockHash, Error> {
        self.try_with_fallback(|client| client.get_block_hash(0))
    }

    /// Get the current chain tip (height + hash).
    ///
    /// Fetches the tip hash first, then resolves its height via `get_block_status` so both
    /// values come from the same point-in-time snapshot, avoiding a TOCTOU mismatch.
    pub fn chain_tip(&self) -> Result<BlockChainTip, Error> {
        let hash = self.try_with_fallback(|client| client.get_tip_hash())?;
        let status = self.try_with_fallback(|client| client.get_block_status(&hash))?;
        let height = status.height.ok_or_else(|| {
            Error::Client(Box::new(esplora_client::Error::HttpResponse {
                status: 404,
                message: format!("tip block {} is not in best chain", hash),
            }))
        })?;
        Ok(BlockChainTip {
            hash,
            height: height as i32,
        })
    }

    /// Get the timestamp of the genesis block (block 0).
    pub fn genesis_block_timestamp(&self) -> Result<u32, Error> {
        let hash = self.genesis_block_hash()?;
        let header = self.try_with_fallback(|client| client.get_header_by_hash(&hash))?;
        Ok(header.time)
    }

    /// Get the timestamp of the current tip block.
    pub fn tip_time(&self) -> Result<u32, Error> {
        let hash = self.try_with_fallback(|client| client.get_tip_hash())?;
        let header = self.try_with_fallback(|client| client.get_header_by_hash(&hash))?;
        Ok(header.time)
    }

    /// Broadcast a transaction to the network.
    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), Error> {
        self.try_with_fallback(|client| client.broadcast(tx))
    }

    /// Perform a sync against the known SPKs.
    ///
    /// `build_request` is called once per attempt (at most twice — primary
    /// then fallback) because BDK's `SyncRequest` is consumed by the call.
    /// The `Box` returned by `EsploraExt::sync` is unboxed inside the closure
    /// so it matches [`Client::try_with_fallback`]'s unboxed-error contract.
    pub fn sync<F>(
        &self,
        mut build_request: F,
        parallel_requests: usize,
    ) -> Result<SyncResult, Error>
    where
        F: FnMut() -> SyncRequest,
    {
        self.try_with_fallback(|client| {
            client
                .sync(build_request(), parallel_requests)
                .map_err(|e| *e)
        })
    }

    /// Perform a full scan from genesis for all keychain SPKs.
    ///
    /// See [`Client::sync`] for the rationale behind the builder closure.
    pub fn full_scan<K, F>(
        &self,
        mut build_request: F,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error>
    where
        K: Ord + Clone + std::fmt::Debug + Send,
        F: FnMut() -> FullScanRequest<K>,
    {
        self.try_with_fallback(|client| {
            client
                .full_scan(build_request(), stop_gap, parallel_requests)
                .map_err(|e| *e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fall_back_classifies_correctly() {
        // Ok results never fall back.
        let ok: Result<(), esplora_client::Error> = Ok(());
        assert!(!should_fall_back(&ok));

        // 402 / 429 fall back (capacity-class 4xx).
        for status in [402u16, 429] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(should_fall_back(&r), "status {} should fall back", status);
        }

        // 5xx falls back.
        for status in [500u16, 502, 503, 504] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(should_fall_back(&r), "status {} should fall back", status);
        }

        // Other 4xx pass through — they describe the request, not the provider.
        for status in [400u16, 401, 403, 404] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(
                !should_fall_back(&r),
                "status {} should not fall back",
                status
            );
        }
    }
}
