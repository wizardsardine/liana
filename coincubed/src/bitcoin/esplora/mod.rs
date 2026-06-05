use std::collections::HashMap;

use bdk_electrum::bdk_chain::{
    bitcoin::{self, bip32::ChildNumber, BlockHash, OutPoint},
    local_chain::{CannotConnectError, LocalChain},
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
    /// Esplora returned a chain update that doesn't connect to the wallet's
    /// existing local_chain. The poller will retry, and `full_scan` has been
    /// flipped on so the next iteration rebuilds from genesis.
    CannotConnect(CannotConnectError),
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
            EsploraError::CannotConnect(e) => write!(
                f,
                "Esplora chain update did not connect to the local chain: '{}'. \
                 Falling back to a full scan.",
                e
            ),
        }
    }
}

/// How often we force a full per-SPK rescan even when the chain tip
/// hasn't moved, as a safety net for mempool-only activity
/// (transactions broadcast to us between blocks, RBF replacements, …).
/// Mempool-only activity is invisible to the [`Esplora::sync_wallet`]
/// tip-guard, so without this cap a quiet chain could let mempool
/// state silently drift out of sync for hours.
///
/// At the default `poll_interval = 600s` (10 min), a value of 6
/// means we do a full rescan at least every hour, capping the
/// mempool-staleness window.
const MAX_POLLS_BEFORE_FORCED_RESCAN: u32 = 6;

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
    /// Chain tip observed at the end of the most recent successful
    /// full sync. The smart-poll guard in [`Self::sync_wallet`]
    /// fetches the current tip on every call and compares it here:
    /// if the chain hasn't advanced *and* we're not at the forced-
    /// rescan boundary, the per-SPK walk is skipped, dropping the
    /// poll cost from ~80 HTTP requests to 1. Set back to `None`
    /// only by a deliberate `trigger_rescan` or on first run.
    last_synced_tip: Option<BlockChainTip>,
    /// Number of polls since the last full per-SPK sync.
    /// Incremented every time the tip-guard short-circuits; reset
    /// to 0 on every actual sync run (including the safety-net
    /// rescans at [`MAX_POLLS_BEFORE_FORCED_RESCAN`]).
    polls_since_full_sync: u32,
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
            last_synced_tip: None,
            polls_since_full_sync: 0,
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
        // Forget the cached tip — the rescan rebuilds the chain
        // from genesis, so any post-rescan tip we observe is
        // genuinely new information and the tip-guard should treat
        // it as such.
        self.last_synced_tip = None;
        self.polls_since_full_sync = 0;
    }

    /// Sync the wallet with the Esplora server. If there was any reorg since the last poll, this
    /// returns the first common ancestor between the previous and the new chain.
    ///
    /// Smart-poll guard: when we're not in a forced full-scan state
    /// and the chain tip we just fetched matches the one cached
    /// from our last full sync, the per-SPK walk is skipped
    /// entirely. This is the common case on a quiet chain and
    /// drops the per-poll cost from ~80 HTTP requests to 1. The
    /// [`MAX_POLLS_BEFORE_FORCED_RESCAN`] safety net forces a full
    /// rescan every Nth idle tick to surface mempool-only activity
    /// (RBF replacements, broadcasts targeting us between blocks)
    /// that the tip-guard would otherwise hide.
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

        // Tip-guard. A single `chain_tip` request is cheap enough
        // (one HTTP call vs. ~80 for the SPK walk) that we always
        // pay it; the savings come from skipping the walk when the
        // tip is unchanged. Only applies when we're not in a
        // forced full-scan and there's a `last_synced_tip` to
        // compare against — first-poll has no baseline.
        if !self.is_rescanning() {
            if let Some(last_tip) = self.last_synced_tip {
                let current_tip = self.client.chain_tip().map_err(EsploraError::Client)?;
                if current_tip == last_tip
                    && self.polls_since_full_sync < MAX_POLLS_BEFORE_FORCED_RESCAN
                {
                    self.polls_since_full_sync = self.polls_since_full_sync.saturating_add(1);
                    log::debug!(
                        "Esplora tip unchanged ({}), skipping per-SPK rescan \
                         (forced rescan in {} more polls)",
                        current_tip,
                        MAX_POLLS_BEFORE_FORCED_RESCAN
                            .saturating_sub(self.polls_since_full_sync),
                    );
                    return Ok(None);
                }
                if current_tip == last_tip {
                    log::debug!(
                        "Esplora tip unchanged ({}) but forced-rescan boundary reached \
                         after {} skipped polls; performing full rescan",
                        current_tip,
                        self.polls_since_full_sync,
                    );
                } else {
                    log::debug!(
                        "Esplora tip advanced from {} to {}; performing full sync",
                        last_tip,
                        current_tip,
                    );
                }
            }
        }

        // Lowered from 4 to 2 to play nicer with mempool.space's
        // per-IP rate window. Hitting 4 concurrent requests against
        // the public mempool tier reliably triggers 429s mid-sync,
        // and a 429 mid-sync is much more expensive than a 2×
        // slowdown on the happy path: BDK throws away the partial
        // result and the next provider in the chain has to redo
        // every SPK from scratch. Two-wide keeps us under mempool's
        // per-second budget more often, so the happy path stays
        // happy.
        const PARALLEL_REQUESTS: usize = 2;
        const STOP_GAP: usize = 200;

        // SPK lists are rebuilt per attempt so the request closure can be
        // re-invoked if the primary Esplora endpoint fails and the client
        // retries on its fallback. BDK's request types are consumed by the
        // call, so the only way to retry is to build a fresh request.
        let (chain_update, mut graph_update, keychain_update) = if !self.is_rescanning() {
            log::debug!("Performing sync.");
            let bdk_wallet = &self.bdk_wallet;
            let local_chain_tip = local_chain_tip.clone();
            let sync_result = self
                .client
                .sync(
                    || -> SyncRequest {
                        let mut request = SyncRequest::from_chain_tip(local_chain_tip.clone());
                        let all_spks: Vec<_> = bdk_wallet
                            .index()
                            .inner()
                            .all_spks()
                            .values()
                            .cloned()
                            .collect();
                        request = request.chain_spks(all_spks);
                        log::debug!("num SPKs for sync: {}", request.spks.len());
                        request
                    },
                    PARALLEL_REQUESTS,
                )
                .map_err(EsploraError::Client)?;
            log::debug!("Sync complete.");
            (sync_result.chain_update, sync_result.graph_update, None)
        } else {
            log::info!("Performing full scan.");
            let bdk_wallet = &self.bdk_wallet;
            let local_chain_tip = local_chain_tip.clone();
            let scan_result = self
                .client
                .full_scan(
                    || {
                        let mut request =
                            FullScanRequest::from_chain_tip(local_chain_tip.clone());
                        for (k, spks) in bdk_wallet.index().all_unbounded_spk_iters() {
                            request = request.set_spks_for_keychain(k, spks);
                        }
                        request
                    },
                    STOP_GAP,
                    PARALLEL_REQUESTS,
                )
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
        let changeset = match self.bdk_wallet.apply_connected_chain_update(chain_update) {
            Ok(cs) => cs,
            Err(e) => {
                // Trigger a full scan on the next poll so the chain rebuilds
                // from genesis and connects unconditionally. The poller
                // already retries on `Err`.
                self.full_scan = true;
                return Err(EsploraError::CannotConnect(e));
            }
        };

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

        // The wallet's local chain is now caught up. Cache its tip so
        // the next poll's tip-guard can short-circuit when the chain
        // hasn't moved, and reset the forced-rescan counter.
        self.last_synced_tip = Some(self.wallet_tip());
        self.polls_since_full_sync = 0;

        Ok(reorg_common_ancestor)
    }

    pub fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.bdk_wallet.get_transaction(txid)
    }
}
