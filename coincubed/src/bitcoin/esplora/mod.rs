use std::collections::HashMap;

use bdk_electrum::bdk_chain::{
    bitcoin::{self, bip32::ChildNumber, BlockHash, OutPoint},
    local_chain::LocalChain,
    spk_client::{FullScanRequest, SyncRequest},
    ChainPosition,
};

pub mod client;

use crate::bitcoin::electrum::{utils, wallet::BdkWallet};
use crate::bitcoin::{Block, BlockChainTip, Coin};

/// An error in the Esplora interface.
#[derive(Debug)]
pub enum EsploraError {
    Client(client::Error),
    GenesisHashMismatch(
        BlockHash, /*expected hash*/
        BlockHash, /*server hash*/
        BlockHash, /*wallet hash*/
    ),
}

impl std::fmt::Display for EsploraError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            EsploraError::Client(e) => write!(f, "Esplora client error: '{}'.", e),
            EsploraError::GenesisHashMismatch(expected, server, wallet) => {
                write!(
                    f,
                    "Genesis hash mismatch. The genesis hash is expected to be '{}'. \
                    The server has hash '{}' and the wallet has hash '{}'.",
                    expected, server, wallet,
                )
            }
        }
    }
}

/// Interface for the Esplora backend.
pub struct Esplora {
    client: client::Client,
    bdk_wallet: BdkWallet,
    /// Used for setting the `last_seen` of unconfirmed transactions in a strictly
    /// increasing manner.
    sync_count: u64,
    /// Set to `true` to force a full scan from the genesis block regardless of
    /// the wallet's local chain height.
    full_scan: bool,
}

impl Esplora {
    pub fn new(
        client: client::Client,
        bdk_wallet: BdkWallet,
        full_scan: bool,
    ) -> Result<Self, EsploraError> {
        Ok(Self {
            client,
            bdk_wallet,
            sync_count: 0,
            full_scan,
        })
    }

    pub fn sanity_checks(&self, expected_hash: &bitcoin::BlockHash) -> Result<(), EsploraError> {
        let server_hash = self
            .client
            .genesis_block_hash()
            .map_err(EsploraError::Client)?;
        let wallet_hash = self.bdk_wallet.local_chain().genesis_hash();
        if server_hash != *expected_hash || wallet_hash != *expected_hash {
            return Err(EsploraError::GenesisHashMismatch(
                *expected_hash,
                server_hash,
                wallet_hash,
            ));
        }
        Ok(())
    }

    pub fn client(&self) -> &client::Client {
        &self.client
    }

    fn local_chain(&self) -> &LocalChain {
        self.bdk_wallet.local_chain()
    }

    /// Get all coins stored in the wallet, taking into consideration only those unconfirmed
    /// transactions that were seen in the last wallet sync.
    pub fn wallet_coins(&self, outpoints: Option<&[OutPoint]>) -> HashMap<OutPoint, Coin> {
        self.bdk_wallet.coins(outpoints, Some(self.sync_count))
    }

    /// Get the tip of the wallet's local chain.
    pub fn wallet_tip(&self) -> BlockChainTip {
        utils::tip_from_block_id(self.local_chain().tip().block_id())
    }

    /// Whether `tip` exists in the wallet's `local_chain`.
    pub fn is_in_wallet_chain(&self, tip: BlockChainTip) -> Option<bool> {
        self.bdk_wallet.is_in_chain(tip)
    }

    /// Whether we'll perform a full scan at the next poll.
    pub fn is_rescanning(&self) -> bool {
        self.full_scan || self.local_chain().tip().height() == 0
    }

    /// Make the poller perform a full scan on the next iteration.
    pub fn trigger_rescan(&mut self) {
        self.full_scan = true;
    }

    /// Sync the wallet with the Esplora server. If there was any reorg since the last poll, this
    /// returns the first common ancestor between the previous and the new chain.
    pub fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, EsploraError> {
        self.bdk_wallet.reveal_spks(receive_index, change_index);
        let local_chain_tip = self.local_chain().tip();
        log::debug!(
            "local chain tip height before sync with esplora: {}",
            local_chain_tip.block_id().height
        );

        const PARALLEL_REQUESTS: usize = 4;
        const STOP_GAP: usize = 200;

        let (chain_update, mut graph_update, keychain_update) = if !self.is_rescanning() {
            log::debug!("Performing sync.");
            let mut request = SyncRequest::from_chain_tip(local_chain_tip.clone());

            let all_spks: Vec<_> = self
                .bdk_wallet
                .index()
                .inner()
                .all_spks()
                .values()
                .cloned()
                .collect();
            request = request.chain_spks(all_spks);
            log::debug!("num SPKs for sync: {}", request.spks.len());

            let sync_result = self
                .client
                .sync(request, PARALLEL_REQUESTS)
                .map_err(EsploraError::Client)?;
            log::debug!("Sync complete.");
            (sync_result.chain_update, sync_result.graph_update, None)
        } else {
            log::info!("Performing full scan.");
            let mut request = FullScanRequest::from_chain_tip(local_chain_tip.clone());

            for (k, spks) in self.bdk_wallet.index().all_unbounded_spk_iters() {
                request = request.set_spks_for_keychain(k, spks);
            }
            let scan_result = self
                .client
                .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
                .map_err(EsploraError::Client)?;
            self.full_scan = false;
            log::info!("Full scan complete.");
            (
                scan_result.chain_update,
                scan_result.graph_update,
                Some(scan_result.last_active_indices),
            )
        };
        log::debug!(
            "chain update height after sync with esplora: {}",
            chain_update.height()
        );

        log::debug!("Full local chain: {:?}", self.local_chain());
        log::debug!("Full chain update: {:?}", chain_update);

        self.sync_count = self.sync_count.checked_add(1).expect("must fit");
        if let Some(keychain_update) = keychain_update {
            self.bdk_wallet.apply_keychain_update(keychain_update);
        }
        let changeset = self.bdk_wallet.apply_connected_chain_update(chain_update);

        let mut changes_iter = changeset.into_iter();
        let reorg_common_ancestor = if let Some((height, _)) = changes_iter.next() {
            if height > local_chain_tip.height() {
                None
            } else {
                log::info!("Block chain reorganization detected.");
                Some(
                    self.bdk_wallet
                        .find_block_before_height(height)
                        .expect("height of first change is greater than 0"),
                )
            }
        } else {
            None
        };

        for tx in &graph_update.initial_changeset().txs {
            let txid = tx.compute_txid();
            if let Some(ChainPosition::Unconfirmed(_)) = graph_update.get_chain_position(
                self.local_chain(),
                self.local_chain().tip().block_id(),
                txid,
            ) {
                log::debug!(
                    "changing last seen for txid '{}' to {}",
                    txid,
                    self.sync_count
                );
                let _ = graph_update.insert_seen_at(txid, self.sync_count);
            }
        }
        self.bdk_wallet.apply_graph_update(graph_update);
        Ok(reorg_common_ancestor)
    }

    pub fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.bdk_wallet.get_transaction(txid)
    }
}
