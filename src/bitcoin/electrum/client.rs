use std::collections::{BTreeMap, HashSet};

use bdk_electrum::{
    bdk_chain::{
        bitcoin,
        local_chain::{CheckPoint, LocalChain},
        spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult},
        BlockId,
    },
    electrum_client::{self, Config, ElectrumApi, HeaderNotification},
    ElectrumExt,
};

use super::utils::{
    block_id_from_tip, height_i32_from_u32, height_i32_from_usize, height_u32_from_i32,
    height_usize_from_i32, height_usize_from_u32, mempool_entry_from_graph,
};
use crate::{
    bitcoin::{BlockChainTip, MempoolEntry},
    config,
};

// If Electrum takes more than 3 minutes to answer one of our queries, fail.
const RPC_SOCKET_TIMEOUT: u8 = 180;

// Number of retries while communicating with the Electrum server.
// A retry happens with exponential back-off (base 2) so this makes us give up after (1+2+4+8+16+32=) 63 seconds.
const RETRY_LIMIT: u8 = 6;

/// An error in the Electrum client.
#[derive(Debug)]
pub enum Error {
    Server(electrum_client::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Server(e) => write!(f, "Electrum error: '{}'.", e),
        }
    }
}

pub struct Client(electrum_client::Client);

impl Client {
    /// Create a new client and perform sanity checks.
    pub fn new(electrum_config: &config::ElectrumConfig) -> Result<Self, Error> {
        let config = Config::builder()
            .retry(RETRY_LIMIT)
            .timeout(Some(RPC_SOCKET_TIMEOUT))
            .build();
        let client =
            bdk_electrum::electrum_client::Client::from_config(&electrum_config.addr, config)
                .map_err(Error::Server)?;
        let ele_client = Self(client);
        Ok(ele_client)
    }

    pub fn chain_tip(&self) -> BlockChainTip {
        let HeaderNotification { height, .. } =
            self.0.block_headers_subscribe().expect("must succeed");
        let new_tip_height = height_i32_from_usize(height);
        let new_tip_hash = self
            .block_hash(new_tip_height)
            .expect("we just fetched this height");
        BlockChainTip {
            height: new_tip_height,
            hash: new_tip_hash,
        }
    }

    pub fn block_hash(&self, height: i32) -> Option<bitcoin::BlockHash> {
        let hash = self
            .0
            .block_header(height_usize_from_i32(height))
            .ok()?
            .block_hash();
        Some(hash)
    }

    pub fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.block_hash(tip.height)
            .map(|bh| bh == tip.hash)
            .unwrap_or(false)
    }

    pub fn genesis_block_timestamp(&self) -> u32 {
        self.0
            .block_header(0)
            .expect("Genesis block must always be there")
            .time
    }

    pub fn genesis_block(&self) -> BlockChainTip {
        let hash = self
            .0
            .block_header(0)
            .expect("Genesis block hash must always be there")
            .block_hash();
        BlockChainTip { hash, height: 0 }
    }

    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<bitcoin::Txid, Error> {
        self.0.transaction_broadcast(tx).map_err(Error::Server)
    }

    pub fn tip_time(&self) -> Option<u32> {
        let tip_height = self.chain_tip().height;
        self.0
            .block_header(height_usize_from_i32(tip_height))
            .ok()
            .map(|bh| bh.time)
    }

    /// Perform the given `SyncRequest` with `ConfirmationTimeHeightAnchor`.
    pub fn sync_with_confirmation_time_height_anchor(
        &self,
        request: SyncRequest,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<SyncResult, Error> {
        self.0
            .sync(request, batch_size, fetch_prev_txouts)
            .map_err(Error::Server)?
            .with_confirmation_time_height_anchor(&self.0)
            .map_err(Error::Server)
    }

    /// Perform the given `FullScanRequest` with `ConfirmationTimeHeightAnchor`.
    pub fn full_scan_with_confirmation_time_height_anchor<K: Ord + Clone>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<FullScanResult<K>, Error> {
        self.0
            .full_scan(request, stop_gap, batch_size, fetch_prev_txouts)
            .map_err(Error::Server)?
            .with_confirmation_time_height_anchor(&self.0)
            .map_err(Error::Server)
    }

    // FIXME: We need to get ancestors & descendants.
    /// Get mempool entry.
    pub fn mempool_entry(&self, txid: &bitcoin::Txid) -> Result<Option<MempoolEntry>, Error> {
        let chain_tip = self.chain_tip();
        let mut local_chain = LocalChain::from_genesis_hash(self.genesis_block().hash).0;
        if chain_tip.height > 0 {
            let _ = local_chain
                .insert_block(block_id_from_tip(chain_tip))
                .expect("only contains genesis block");
        }
        let request = SyncRequest::from_chain_tip(local_chain.tip()).chain_txids(vec![*txid]);
        let sync_result = self
            .0
            .sync(request, 10, true)
            .map_err(Error::Server)?
            .with_confirmation_time_height_anchor(&self.0)
            .map_err(Error::Server)?;
        let graph = sync_result.graph_update;
        let entry = mempool_entry_from_graph(&graph, &local_chain, txid);
        Ok(entry)
    }

    // FIXME: We need to get ancestors & descendants.
    /// Get mempool spenders of the given outpoints.
    pub fn mempool_spenders(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<MempoolEntry>, Error> {
        let chain_tip = self.chain_tip();
        let mut local_chain = LocalChain::from_genesis_hash(self.genesis_block().hash).0;
        if chain_tip.height > 0 {
            let _ = local_chain
                .insert_block(block_id_from_tip(chain_tip))
                .expect("only contains genesis block");
        }
        let request =
            SyncRequest::from_chain_tip(local_chain.tip()).chain_outpoints(outpoints.to_vec());
        let sync_result = self
            .0
            .sync(request, 10, true)
            .map_err(Error::Server)?
            .with_confirmation_time_height_anchor(&self.0)
            .map_err(Error::Server)?;
        let graph = sync_result.graph_update;
        let txids: HashSet<_> = outpoints
            .iter()
            .flat_map(|op| graph.outspends(*op))
            .collect();
        let mut entries = Vec::new();
        for txid in txids {
            let entry = mempool_entry_from_graph(&graph, &local_chain, txid);
            if let Some(entry) = entry {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Get the block in `local_chain` that `tip` has in common with the Electrum server.
    pub fn common_ancestor(
        &self,
        local_chain: &LocalChain,
        tip: &BlockChainTip,
    ) -> Option<BlockChainTip> {
        let server_tip_height = self.chain_tip().height as u32;
        let tip_block = BlockId {
            hash: tip.hash,
            height: height_u32_from_i32(tip.height),
        };
        // Get a local chain that includes all our checkpoints up to and including `tip`.
        // Typically, the local chain's tip should be the same as `tip`, but the local chain's tip
        // may have advanced slightly in case the Electrum tip changed while performing a round of
        // updates and we restarted.
        let trunc_chain = {
            let mut chain = local_chain.clone();
            // We want to disconnect all checkpoints *after* `tip`, but `disconnect_from` is inclusive.
            // So call `disconnect_from` and then re-insert `tip`.

            // We can only get an error if `tip` is the genesis block, but we should never
            // be trying to find the common ancestor of the genesis block.
            let _ = chain
                .disconnect_from(tip_block)
                .expect("we should not be trying to find common ancestor with genesis block");
            let _ = chain
                .insert_block(tip_block)
                .expect("we have already removed this block from chain");
            chain
        };
        // The following code is based on the function `construct_update_tip`. See:
        // https://github.com/bitcoindevkit/bdk/blob/4a8452f9b8f8128affbb60665016fedb48f07cd6/crates/electrum/src/electrum_ext.rs#L284
        // TODO: Is the following comment and code correct in our case? Could the electrum tip be lower and a reorged chain?

        // If electrum returns a tip height that is lower than our previous tip, then checkpoints do
        // not need updating. We just return the previous tip and use that as the point of agreement.
        // if new_tip_height < prev_tip.height() {
        //     return Ok((prev_tip.clone(), Some(prev_tip.height())));
        // }

        const CHAIN_SUFFIX_LENGTH: u32 = 8;
        // Atomically fetch the latest `CHAIN_SUFFIX_LENGTH` count of blocks from Electrum. We use this
        // to construct our checkpoint update.
        let mut new_blocks = {
            let start_height = server_tip_height.saturating_sub(CHAIN_SUFFIX_LENGTH - 1);
            let hashes = self
                .0
                .block_headers(
                    height_usize_from_u32(start_height),
                    CHAIN_SUFFIX_LENGTH as _,
                )
                .ok()?
                .headers
                .into_iter()
                .map(|h| h.block_hash());
            (start_height..).zip(hashes).collect::<BTreeMap<u32, _>>()
        };

        // Find the "point of agreement" (if any).
        let agreement_cp = {
            let mut agreement_cp = Option::<CheckPoint>::None;
            for cp in trunc_chain
                .tip()
                .iter()
                .filter(|cp| cp.height() <= server_tip_height)
            {
                let cp_block = cp.block_id();
                let hash = match new_blocks.get(&cp_block.height) {
                    Some(&hash) => hash,
                    None => {
                        assert!(
                            cp_block.height <= server_tip_height,
                            "already checked that server tip cannot be smaller"
                        );
                        let hash = self
                            .0
                            .block_header(height_usize_from_u32(cp_block.height))
                            .ok()?
                            .block_hash();
                        new_blocks.insert(cp_block.height, hash);
                        hash
                    }
                };
                if hash == cp_block.hash {
                    agreement_cp = Some(cp);
                    break;
                }
            }
            agreement_cp
        };
        agreement_cp.as_ref().map(|cp| BlockChainTip {
            height: height_i32_from_u32(cp.height()),
            hash: cp.hash(),
        })
    }
}
