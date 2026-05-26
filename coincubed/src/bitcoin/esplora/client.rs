use bdk_electrum::bdk_chain::{
    bitcoin::{self, BlockHash},
    spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult},
};
use bdk_esplora::{esplora_client, EsploraExt};

use crate::bitcoin::BlockChainTip;

const REQUEST_TIMEOUT_SECS: u64 = 90;

/// An error from the Esplora client.
#[derive(Debug)]
pub enum Error {
    Client(Box<esplora_client::Error>),
    /// The server's genesis block hash does not match the expected hash for the
    /// configured Bitcoin network. Catches misconfigurations like pointing a
    /// Signet wallet at Mutinynet at the earliest possible moment, before any
    /// wallet state is built or synced.
    GenesisHashMismatch {
        expected: BlockHash,
        server: BlockHash,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Client(e) => write!(f, "Esplora client error: '{}'.", e),
            Error::GenesisHashMismatch { expected, server } => write!(
                f,
                "Esplora server returned genesis hash '{}', but the configured network expects '{}'. \
                 The Esplora URL does not match the wallet's network.",
                server, expected,
            ),
        }
    }
}

pub struct Client(esplora_client::blocking::BlockingClient);

impl Client {
    /// Create a new client and verify connectivity by fetching the current tip height.
    ///
    /// If `expected_genesis` is provided, also verify the server's genesis block hash
    /// matches it — this rejects mismatched-network URLs (e.g. Mutinynet for a Signet
    /// wallet) before any wallet state is built.
    pub fn new(
        config: &crate::config::EsploraConfig,
        expected_genesis: Option<BlockHash>,
    ) -> Result<Self, Error> {
        let addr = normalize_esplora_base_url(&config.addr);
        let mut builder = esplora_client::Builder::new(&addr).timeout(REQUEST_TIMEOUT_SECS);
        // The blocking esplora client uses `minreq` underneath, which has no
        // content-encoding support. If the server returns gzip/brotli-compressed
        // bodies (common for /address/:addr/txs and other large responses),
        // minreq tries to read the raw bytes as UTF-8 and fails with
        // `InvalidUtf8InResponse`. Force the server to send uncompressed
        // responses to avoid this.
        builder = builder.header("Accept-Encoding", "identity");
        if let Some(token) = &config.token {
            builder = builder.header("Authorization", &format!("Bearer {}", token));
        }
        let inner = builder.build_blocking();
        // Verify we can reach the server.
        inner.get_height().map_err(|e| Error::Client(Box::new(e)))?;
        if let Some(expected) = expected_genesis {
            let server = inner
                .get_block_hash(0)
                .map_err(|e| Error::Client(Box::new(e)))?;
            if server != expected {
                return Err(Error::GenesisHashMismatch { expected, server });
            }
        }
        Ok(Client(inner))
    }

    /// Get the genesis block hash (block at height 0).
    pub fn genesis_block_hash(&self) -> Result<bitcoin::BlockHash, Error> {
        self.0
            .get_block_hash(0)
            .map_err(|e| Error::Client(Box::new(e)))
    }

    /// Get the current chain tip (height + hash).
    ///
    /// Fetches the tip hash first, then resolves its height via `get_block_status` so both
    /// values come from the same point-in-time snapshot, avoiding a TOCTOU mismatch.
    pub fn chain_tip(&self) -> Result<BlockChainTip, Error> {
        let hash = self
            .0
            .get_tip_hash()
            .map_err(|e| Error::Client(Box::new(e)))?;
        let status = self
            .0
            .get_block_status(&hash)
            .map_err(|e| Error::Client(Box::new(e)))?;
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
        let header = self
            .0
            .get_header_by_hash(&hash)
            .map_err(|e| Error::Client(Box::new(e)))?;
        Ok(header.time)
    }

    /// Get the timestamp of the current tip block.
    pub fn tip_time(&self) -> Result<u32, Error> {
        let hash = self
            .0
            .get_tip_hash()
            .map_err(|e| Error::Client(Box::new(e)))?;
        let header = self
            .0
            .get_header_by_hash(&hash)
            .map_err(|e| Error::Client(Box::new(e)))?;
        Ok(header.time)
    }

    /// Broadcast a transaction to the network.
    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), Error> {
        self.0.broadcast(tx).map_err(|e| Error::Client(Box::new(e)))
    }

    /// Perform a sync against the known SPKs.
    pub fn sync(
        &self,
        request: SyncRequest,
        parallel_requests: usize,
    ) -> Result<SyncResult, Error> {
        self.0
            .sync(request, parallel_requests)
            .map_err(Error::Client)
    }

    /// Perform a full scan from genesis for all keychain SPKs.
    pub fn full_scan<K: Ord + Clone + std::fmt::Debug + Send>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error> {
        self.0
            .full_scan(request, stop_gap, parallel_requests)
            .map_err(Error::Client)
    }
}

fn normalize_esplora_base_url(addr: &str) -> String {
    let mut addr = addr.trim().trim_end_matches('/').to_string();
    for marker in [
        "/blocks/",
        "/block/",
        "/tx/",
        "/address/",
        "/scripthash/",
        "/mempool",
        "/fee-estimates",
    ] {
        if let Some(pos) = addr.find(marker) {
            addr.truncate(pos);
            return addr.trim_end_matches('/').to_string();
        }
    }
    addr
}
