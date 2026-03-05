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

pub struct Client(esplora_client::blocking::BlockingClient);

impl Client {
    /// Create a new client and verify connectivity by fetching the current tip height.
    pub fn new(config: &crate::config::EsploraConfig) -> Result<Self, Error> {
        let mut builder = esplora_client::Builder::new(&config.addr).timeout(REQUEST_TIMEOUT_SECS);
        if let Some(token) = &config.token {
            builder = builder.header("Authorization", &format!("Bearer {}", token));
        }
        let inner = builder.build_blocking();
        // Verify we can reach the server.
        inner.get_height().map_err(|e| Error::Client(Box::new(e)))?;
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
