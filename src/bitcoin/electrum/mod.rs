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
}

impl Electrum {
    pub fn new(
        client: client::Client,
        bdk_wallet: wallet::BdkWallet,
    ) -> Result<Self, ElectrumError> {
        Ok(Self { client, bdk_wallet })
    }

    pub fn sanity_checks(&self, expected_hash: &bitcoin::BlockHash) -> Result<(), ElectrumError> {
        let server_hash = self.client.genesis_block().hash;
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

    pub fn wallet_coins(&self, outpoints: Option<&[OutPoint]>) -> HashMap<OutPoint, Coin> {
        self.bdk_wallet.coins(outpoints)
    }

    /// Sync the wallet with the Electrum server.
    pub fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<(), ElectrumError> {
        self.bdk_wallet.reveal_spks(receive_index, change_index);
        let chain_tip = self.local_chain().tip();
        log::debug!(
            "local chain tip height before sync with electrum: {}",
            chain_tip.block_id().height
        );

        const BATCH_SIZE: usize = 200;
        // We'll only need to calculate fees of mempool transactions and this will be done separately from our graph
        // so we don't need to fetch prev txouts. In any case, we'll already have these for our own transactions.
        const FETCH_PREV_TXOUTS: bool = false;
        const STOP_GAP: usize = 50;

        let (chain_update, mut graph_update, keychain_update) = if chain_tip.height() > 0 {
            log::info!("Performing sync.");
            let mut request = SyncRequest::from_chain_tip(chain_tip.clone())
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
                .sync_with_confirmation_time_height_anchor(request, BATCH_SIZE, FETCH_PREV_TXOUTS)
                .map_err(ElectrumError::Client)?;
            (sync_result.chain_update, sync_result.graph_update, None)
        } else {
            log::info!("Performing full scan.");
            let mut request = FullScanRequest::from_chain_tip(chain_tip.clone())
                .cache_graph_txs(self.bdk_wallet.graph());

            for (k, spks) in self.bdk_wallet.index().all_unbounded_spk_iters() {
                request = request.set_spks_for_keychain(k, spks);
            }
            let scan_result = self
                .client
                .full_scan_with_confirmation_time_height_anchor(
                    request,
                    STOP_GAP,
                    BATCH_SIZE,
                    FETCH_PREV_TXOUTS,
                )
                .map_err(ElectrumError::Client)?;
            (
                scan_result.chain_update,
                scan_result.graph_update,
                Some(scan_result.last_active_indices),
            )
        };
        if let Some(keychain_update) = keychain_update {
            self.bdk_wallet.apply_keychain_update(keychain_update);
        }
        log::debug!(
            "local chain tip height after sync with electrum: {}",
            chain_update.height()
        );
        self.bdk_wallet.apply_connected_chain_update(chain_update);

        // Unconfirmed transactions have their last seen as 0, so we override to the current time
        // so that conflicts can be properly handled.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("must be greater than unix epoch")
            .as_secs();

        for tx in &graph_update.initial_changeset().txs {
            let txid = tx.txid();
            if let Some(ChainPosition::Unconfirmed(last_seen)) = graph_update.get_chain_position(
                self.local_chain(),
                self.local_chain().tip().block_id(),
                txid,
            ) {
                let prev_last_seen = if let Some(ChainPosition::Unconfirmed(last_seen)) =
                    self.bdk_wallet.graph().get_chain_position(
                        self.local_chain(),
                        self.local_chain().tip().block_id(),
                        txid,
                    ) {
                    last_seen
                } else {
                    last_seen
                };
                log::debug!(
                    "changing last seen for txid '{}' from {} to {}",
                    txid,
                    prev_last_seen,
                    now
                );
                let _ = graph_update.insert_seen_at(txid, now);
            }
        }
        self.bdk_wallet.apply_graph_update(graph_update);
        Ok(())
    }

    pub fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        self.client.common_ancestor(self.local_chain(), tip)
    }

    pub fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.bdk_wallet.get_transaction(txid)
    }
}
