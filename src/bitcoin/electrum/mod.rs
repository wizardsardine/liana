use std::collections::HashMap;

use bdk_electrum::bdk_chain::{
    bitcoin::{self, bip32::ChildNumber, BlockHash, OutPoint},
    local_chain::LocalChain,
    spk_client::{FullScanRequest, SyncRequest},
    ChainPosition,
};

pub mod client;
mod utils;
pub mod wallet;
use crate::bitcoin::{Block, BlockChainTip, Coin};

/// An error in the Electrum interface.
#[derive(Debug)]
pub enum ElectrumError {
    Client(client::Error),
    GenesisHashMismatch(
        BlockHash, /*expected hash*/
        BlockHash, /*server hash*/
        BlockHash, /*wallet hash*/
    ),
}

impl std::fmt::Display for ElectrumError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ElectrumError::Client(e) => write!(f, "Electrum client error: '{}'.", e),
            ElectrumError::GenesisHashMismatch(expected, server, wallet) => {
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

/// Interface for Electrum backend.
pub struct Electrum {
    client: client::Client,
    bdk_wallet: wallet::BdkWallet,
    /// Used for setting the `last_seen` of unconfirmed transactions in a strictly
    /// increasing manner.
    sync_count: u64,
    /// Set to `true` to force a full scan from the genesis block regardless of
    /// the wallet's local chain height.
    full_scan: bool,
}

impl Electrum {
    pub fn new(
        client: client::Client,
        bdk_wallet: wallet::BdkWallet,
        full_scan: bool,
    ) -> Result<Self, ElectrumError> {
        Ok(Self {
            client,
            bdk_wallet,
            sync_count: 0,
            full_scan,
        })
    }

    pub fn sanity_checks(&self, expected_hash: &bitcoin::BlockHash) -> Result<(), ElectrumError> {
        let server_hash = self
            .client
            .genesis_block()
            .map_err(ElectrumError::Client)?
            .hash;
        let wallet_hash = self.bdk_wallet.local_chain().genesis_hash();
        if server_hash != *expected_hash || wallet_hash != *expected_hash {
            return Err(ElectrumError::GenesisHashMismatch(
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
    ///
    /// Returns `None` if no block at that height exists in `local_chain`.
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

    /// Sync the wallet with the Electrum server. If there was any reorg since the last poll, this
    /// returns the first common ancestor between the previous and the new chain.
    pub fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, ElectrumError> {
        self.bdk_wallet.reveal_spks(receive_index, change_index);
        let local_chain_tip = self.local_chain().tip();
        log::debug!(
            "local chain tip height before sync with electrum: {}",
            local_chain_tip.block_id().height
        );

        // We'll only need to calculate fees of mempool transactions and this will be done separately from our graph
        // so we don't need to fetch prev txouts. In any case, we'll already have these for our own transactions.
        const FETCH_PREV_TXOUTS: bool = false;
        const STOP_GAP: usize = 50;

        let (chain_update, mut graph_update, keychain_update) = if !self.is_rescanning() {
            log::info!("Performing sync.");
            let mut request = SyncRequest::from_chain_tip(local_chain_tip.clone())
                .cache_graph_txs(self.bdk_wallet.graph());

            let all_spks: Vec<_> = self
                .bdk_wallet
                .index()
                .inner() // we include lookahead SPKs
                .all_spks()
                .iter()
                .map(|(_, script)| script.clone())
                .collect();
            request = request.chain_spks(all_spks);
            log::debug!("num SPKs for sync: {}", request.spks.len());

            let sync_result = self
                .client
                .sync_with_confirmation_time_height_anchor(request, FETCH_PREV_TXOUTS)
                .map_err(ElectrumError::Client)?;
            log::info!("Sync complete.");
            (sync_result.chain_update, sync_result.graph_update, None)
        } else {
            log::info!("Performing full scan.");
            // Either local_chain has height 0 or we want to trigger a full scan.
            // In both cases, the scan should be from the genesis block.
            let genesis_block = local_chain_tip.get(0).expect("must contain genesis block");
            let mut request = FullScanRequest::from_chain_tip(genesis_block)
                .cache_graph_txs(self.bdk_wallet.graph());

            for (k, spks) in self.bdk_wallet.index().all_unbounded_spk_iters() {
                request = request.set_spks_for_keychain(k, spks);
            }
            let scan_result = self
                .client
                .full_scan_with_confirmation_time_height_anchor(
                    request,
                    STOP_GAP,
                    FETCH_PREV_TXOUTS,
                )
                .map_err(ElectrumError::Client)?;
            // A full scan only makes sense to do once, in most cases. Don't do it again unless
            // explicitly asked to by a user.
            self.full_scan = false;
            log::info!("Full scan complete.");
            (
                scan_result.chain_update,
                scan_result.graph_update,
                Some(scan_result.last_active_indices),
            )
        };
        log::debug!(
            "chain update height after sync with electrum: {}",
            chain_update.height()
        );

        // Increment the sync count and apply changes.
        self.sync_count = self.sync_count.checked_add(1).expect("must fit");
        if let Some(keychain_update) = keychain_update {
            self.bdk_wallet.apply_keychain_update(keychain_update);
        }
        let changeset = self.bdk_wallet.apply_connected_chain_update(chain_update);

        let mut changes_iter = changeset.into_iter();
        let reorg_common_ancestor = if let Some((height, _)) = changes_iter.next() {
            // Either a new block has been added at this height or an existing block in our local
            // chain has been invalidated.
            // Since we iterate in ascending height order, we'll see the lowest block height first.
            // If the lowest height is higher than our height before syncing, we're good.
            // Else if it's adding/invalidating a block at height before syncing or lower,
            // it's a reorg.
            if height > local_chain_tip.height() {
                None
            } else {
                log::info!("Block chain reorganization detected.");
                // We can assume height is positive as genesis block will not have changed.
                Some(
                    self.bdk_wallet
                        .find_block_before_height(height)
                        .expect("height of first change is greater than 0"),
                )
            }
        } else {
            None
        };

        // Unconfirmed transactions have their last seen as 0, so we override to the `sync_count`
        // so that conflicts can be properly handled. We use `sync_count` instead of current time
        // in seconds to ensure strictly increasing values between poller iterations.
        for tx in &graph_update.initial_changeset().txs {
            let txid = tx.txid();
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
