//! Interface to the Bitcoin network.
//!
//! Broadcast transactions, poll for new unspent coins, gather fee estimates.

pub mod d;
pub mod electrum;
pub mod esplora;
pub mod poller;

use crate::bitcoin::d::{BitcoindError, CachedTxGetter, LSBlockEntry};
use coincube_core::descriptors;
pub use d::{MempoolEntry, MempoolEntryFees, SyncProgress};

use std::{collections::HashMap, fmt, sync};

use miniscript::bitcoin::{self, address, bip32::ChildNumber};

// A spent coin's outpoint together with its spend transaction's txid, height and time.
type SpentCoin = (bitcoin::OutPoint, bitcoin::Txid, i32, u32);

const COINBASE_MATURITY: i32 = 100;

/// Information about a block
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct Block {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
    pub time: u32,
}

/// Information about the best block in the chain
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BlockChainTip {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
}

impl fmt::Display for BlockChainTip {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({},{})", self.height, self.hash)
    }
}

/// Outcome of walking back from our tip to where it rejoins the backend's chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncestorSearch {
    /// The fork point, found within the allowed depth.
    Found(BlockChainTip),
    /// The walk hit its bound without rejoining, so the fork is at least that deep.
    /// Distinct from [`Self::Failed`] because it is an answer, not an absence of
    /// one: the caller should reject the reorg rather than retry the lookup.
    TooDeep,
    /// The backend could not answer.
    Failed,
}

/// Lock-free cache of the latest [`SyncProgress`], published by the poller and
/// read by `get_info` WITHOUT taking the `BitcoinInterface` mutex.
///
/// The poller holds that mutex across a full wallet scan — over Esplora that's
/// a long sequence of HTTP requests — and `get_info` → `sync_progress` needs
/// the same lock, so a fresh vault's first scan left `get_info` (and the GUI's
/// "Starting daemon…" gate, which awaits it) blocked for the entire scan.
/// Routing the read through these atomics decouples it from that lock. The
/// three fields are stored independently, so a concurrent reader can briefly
/// observe a mix of two updates — harmless for a progress indicator, which is
/// re-read every poll.
#[derive(Debug, Default)]
pub struct SyncProgressCache {
    percentage_bits: sync::atomic::AtomicU64,
    headers: sync::atomic::AtomicU64,
    blocks: sync::atomic::AtomicU64,
}

impl SyncProgressCache {
    /// Publish the latest progress. Called by the poller right after it reads
    /// `sync_progress` from the backend (with the backend lock already
    /// released), so this never runs while the `BitcoinInterface` mutex is held.
    pub fn store(&self, progress: &SyncProgress) {
        use sync::atomic::Ordering::Relaxed;
        self.percentage_bits
            .store(progress.percentage().to_bits(), Relaxed);
        self.headers.store(progress.headers, Relaxed);
        self.blocks.store(progress.blocks, Relaxed);
    }

    /// Read the latest published progress without locking. Before the poller's
    /// first publish this returns the zero default (0%), which reads as
    /// "still syncing" — exactly the right answer during startup.
    pub fn load(&self) -> SyncProgress {
        use sync::atomic::Ordering::Relaxed;
        SyncProgress::new(
            f64::from_bits(self.percentage_bits.load(Relaxed)),
            self.headers.load(Relaxed),
            self.blocks.load(Relaxed),
        )
    }
}

/// A chain-sync alert the poller raises when it deliberately stops updating our view
/// of the chain from the backend. Two distinct conditions lead here, and they must not
/// be conflated — see the variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainAlert {
    /// No outstanding alert: our chain view is tracking the backend's.
    None,
    /// The backend reported a reorganisation deeper than `MAX_REORG_DEPTH` and we
    /// refused to apply it. That almost certainly means the backend is misreporting
    /// rather than that Bitcoin genuinely undid that much history (a node rewound with
    /// `invalidateblock`, a datadir swapped underneath us, a chainstate mid-rebuild):
    /// applying it would clear the confirmation state of every coin above the ancestor
    /// and hard-delete any deposit no longer in the mempool. The value is the refused
    /// depth in blocks — a *lower bound* when the ancestor walk stopped at its limit.
    RefusedReorg(i32),
    /// Our tip is off the backend's chain and this backend (Electrum/Esplora) cannot be
    /// asked where the two chains parted. There is no known fork point and therefore no
    /// rollback depth to report — the height gap between the tips is *not* one, and may
    /// be zero or negative even though sync is genuinely paused. Wallet state is held
    /// intact until the chains reconverge.
    Diverged,
}

/// Lock-free cache of the poller's current [`ChainAlert`], read by `get_info` without
/// taking the `BitcoinInterface` mutex (same rationale as [`SyncProgressCache`]).
///
/// A `Some(..)`-flavoured alert means our view of the chain is deliberately not being
/// updated from the backend. The state is kept in a single atomic so a reader never
/// observes a torn mix of the two conditions:
///   * `0` — no alert;
///   * `> 0` — a refused reorg, the value being its depth in blocks;
///   * [`DIVERGED_SENTINEL`] — an unresolved chain divergence, which carries no depth.
#[derive(Debug, Default)]
pub struct ReorgAlertCache {
    state: sync::atomic::AtomicI64,
}

/// Reserved slot value standing for [`ChainAlert::Diverged`]. Out of range of any real
/// reorg depth (always a positive block count), so it can never collide with one.
const DIVERGED_SENTINEL: i64 = i64::MIN;

impl ReorgAlertCache {
    /// Record that a reorg of `depth` blocks was refused as implausibly deep.
    pub fn store_refused_reorg(&self, depth: i32) {
        self.state
            .store(depth.into(), sync::atomic::Ordering::Relaxed);
    }

    /// Record that our tip has diverged from a backend that cannot be asked where the
    /// chains parted. Deliberately carries no depth: the fork point is unknown, and
    /// publishing the tips' height gap here would misrepresent an unknown divergence as
    /// an exact rollback depth.
    pub fn store_divergence(&self) {
        self.state
            .store(DIVERGED_SENTINEL, sync::atomic::Ordering::Relaxed);
    }

    /// Clear the alert. Called on every poll that completes normally, so both a
    /// transient deep-reorg misreport and a divergence resolve themselves once the
    /// backend's chain contains our tip again.
    pub fn clear(&self) {
        self.state.store(0, sync::atomic::Ordering::Relaxed);
    }

    /// The outstanding alert, if any.
    pub fn load(&self) -> ChainAlert {
        match self.state.load(sync::atomic::Ordering::Relaxed) {
            0 => ChainAlert::None,
            DIVERGED_SENTINEL => ChainAlert::Diverged,
            depth => ChainAlert::RefusedReorg(depth as i32),
        }
    }
}

/// Set while the shared managed node is being deliberately rewound, so pollers
/// know to stand down instead of reacting to a chain they are being shown
/// mid-surgery.
///
/// Process-wide because the managed node is process-wide: every Vault runs its own
/// poller against the one node, so a rewind driven by one Vault's settings screen
/// has to quiesce all of them. A per-daemon channel would only reach the Vault that
/// started it. (Same reasoning as the process-wide managed-Tor registry.)
///
/// This is a courtesy, not a safety mechanism. It stops pollers from *starting*
/// work; it cannot stop one already in flight. What actually protects wallet state
/// is the depth guard in the poller, which refuses an implausible rollback whether
/// or not this flag is set.
static MANAGED_NODE_MAINTENANCE: sync::atomic::AtomicBool = sync::atomic::AtomicBool::new(false);

/// Mark the shared managed node as under (or no longer under) maintenance.
///
/// Callers MUST clear this on every exit path, including failures — a flag left
/// set silently stops every Vault from updating. Prefer [`MaintenanceGuard`].
pub fn set_managed_node_maintenance(active: bool) {
    MANAGED_NODE_MAINTENANCE.store(active, sync::atomic::Ordering::SeqCst);
}

/// Whether the shared managed node is currently under maintenance.
pub fn managed_node_maintenance() -> bool {
    MANAGED_NODE_MAINTENANCE.load(sync::atomic::Ordering::SeqCst)
}

/// RAII wrapper around the maintenance flag, so an early return or a panic cannot
/// leave every Vault's poller parked forever.
///
/// Acquired atomically, so it doubles as a mutual exclusion for the operation it
/// guards: several Vaults can attach to the shared node at the same instant, and
/// only one may rewind it.
pub struct MaintenanceGuard;

impl MaintenanceGuard {
    /// Claim maintenance, or `None` if another holder already has it.
    ///
    /// A compare-and-swap rather than a check followed by a set: the two Vaults this
    /// exists to separate can arrive in the same instant, which is exactly when a
    /// read-then-write loses.
    pub fn try_acquire() -> Option<Self> {
        MANAGED_NODE_MAINTENANCE
            .compare_exchange(
                false,
                true,
                sync::atomic::Ordering::SeqCst,
                sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
            .then_some(MaintenanceGuard)
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        set_managed_node_maintenance(false);
    }
}

/// The floor of a rollback we deliberately caused, which the poller may therefore
/// apply even though it is deeper than its limit.
///
/// The depth guard exists to distrust the *backend*: past a few hundred blocks, a
/// node claiming that much history was undone is far likelier to be misreporting
/// than right. A managed-node repair breaks that assumption — we asked for the
/// rewind and we chose how deep it went — and without an exception the guard is
/// permanent: maintenance ends, every later poll sees the same over-deep reorg,
/// refuses it, and the Vault stays pinned to a chain the node no longer has.
///
/// A floor rather than a single block, because the fork point is not knowable in
/// advance. A repair rewinds to this block and lets the node reconnect from there;
/// where the two chains part company is wherever the node first refuses a block,
/// which can be anywhere above the floor. Rewinding to 961,631 and rejecting
/// 966,000 leaves the chains sharing everything up to 965,999, and it is *that*
/// block the poller will find — so pinning the exception to the floor would refuse
/// every realistic outcome and authorise only the one where nothing replayed.
///
/// The hash is still carried, and still checked: the poller confirms the backend's
/// chain actually contains this block before honouring the floor, so a sanction
/// left over from a different chain or a different datadir authorises nothing.
static SANCTIONED_ROLLBACK: sync::Mutex<Option<BlockChainTip>> = sync::Mutex::new(None);

/// Authorise (or withdraw) over-deep rollbacks reaching no deeper than `floor`.
pub fn set_sanctioned_rollback(floor: Option<BlockChainTip>) {
    *SANCTIONED_ROLLBACK
        .lock()
        .expect("sanctioned rollback lock poisoned") = floor;
}

/// The floor the poller is currently allowed to roll back to past its depth limit.
pub fn sanctioned_rollback() -> Option<BlockChainTip> {
    *SANCTIONED_ROLLBACK
        .lock()
        .expect("sanctioned rollback lock poisoned")
}

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    /// Whether this backend talks to a `bitcoind`.
    ///
    /// Used to scope the managed-node maintenance pause: an Esplora or Electrum
    /// Vault is unaffected by a rewind of the managed node and must keep polling.
    /// An external `bitcoind` we cannot distinguish from the managed one, so it
    /// pauses too — conservative, and bounded by the rewind's duration.
    fn is_bitcoind(&self) -> bool {
        false
    }

    fn genesis_block_timestamp(&self) -> u32;

    fn genesis_block(&self) -> BlockChainTip;

    /// Get the progress of the block chain synchronization.
    /// Returns a rounded up percentage between 0 and 1. Use the `is_synced` method to be sure the
    /// backend is completely synced to the best known tip.
    fn sync_progress(&self) -> SyncProgress;

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Get the timestamp set in the best block's header.
    fn tip_time(&self) -> Option<u32>;

    /// Check whether this former tip is part of the current best chain.
    fn is_in_chain(&self, tip: &BlockChainTip) -> bool;

    /// Sync the wallet with the current best chain.
    /// `receive_index` and `change_index` are the last derivation indices
    /// that are expected to have been used by the wallet.
    /// In case there has been a reorg, returns the common ancestor between
    /// the wallet and the reorged chain.
    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String>;

    /// Get coins received since the specified tip.
    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathCoincubeDesc],
    ) -> Vec<UTxO>;

    /// Get all coins that were confirmed, and at what height and time. Along with "expired"
    /// unconfirmed coins (for instance whose creating transaction may have been replaced).
    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>);

    /// Get all coins that are being spent, and the spending txid.
    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)>;

    /// Get all coins that are spent with the final spend tx txid and blocktime. Along with the
    /// coins for which the spending transaction "expired" (a conflicting transaction was mined and
    /// it wasn't spending this coin).
    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>);

    /// Get the common ancestor between the Bitcoin backend's tip and the given tip.
    /// Walk back from `tip` until it rejoins our chain, giving up after `max_depth`
    /// steps.
    ///
    /// The bound matters: this costs one backend round-trip per block, so an
    /// unbounded walk lets a misreporting backend extract tens of thousands of
    /// sequential requests from us before the caller gets a chance to reject the
    /// result. `TooDeep` lets the caller refuse without paying for the rest.
    fn common_ancestor(&self, tip: &BlockChainTip, max_depth: i32) -> AncestorSearch;

    /// Whether [`Self::common_ancestor`] can answer at all for this backend.
    ///
    /// Backends that hand us the fork point straight from [`Self::sync_wallet`]
    /// (Electrum, Esplora) cannot walk back for one: all they hold is a chain of
    /// their own, with no record of the chain *we* were on to compare it against.
    /// Callers MUST check this before reaching for `common_ancestor`.
    ///
    /// The invariant that used to make asking them unthinkable — `sync_wallet`
    /// reports every reorg, so the poller's own reorg branch is dead code on those
    /// backends — does not survive the poller *refusing* a reorg: our tip then stays
    /// diverged from the backend's, and every later poll re-enters that branch with
    /// no reorg being reported to explain it. Their `common_ancestor` was an
    /// `unreachable!()` in a thread of a `panic = "abort"` binary, so the first poll
    /// after a refusal took the whole app down with it.
    fn walks_common_ancestor(&self) -> bool {
        true
    }

    /// Broadcast this transaction to the Bitcoin P2P network
    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String>;

    /// Request that the next [`Self::sync_wallet`] call bypass any
    /// short-circuit optimisations (e.g. the smart-poll tip-guard
    /// on the Esplora backend) and do a full sync against the
    /// providers. No-op for backends that don't have such an
    /// optimisation, so the default impl is fine for everyone but
    /// Esplora.
    ///
    /// The GUI calls this — via the `requestsync` JSON-RPC — when
    /// the user does something that signals "I want fresh data
    /// now": app regains focus, the Receive panel opens, a new
    /// receive address is generated, etc. The flag is consumed on
    /// the next sync, so subsequent ticks resume normal smart-poll
    /// behaviour.
    fn request_eager_sync(&mut self) {}

    /// Trigger a rescan of the block chain for transactions related to this descriptor since
    /// the given date.
    fn start_rescan(
        &mut self,
        desc: &descriptors::CoincubeDescriptor,
        timestamp: u32,
    ) -> Result<(), String>;

    /// Rescan progress percentage. Between 0 and 1.
    fn rescan_progress(&self) -> Option<f64>;

    /// Get the last block chain tip with a timestamp below this. Timestamp must be a valid block
    /// timestamp.
    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip>;

    /// Get a transaction related to the wallet along with potential confirmation info.
    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)>;

    /// Get the details of unconfirmed transactions spending these outpoints, if any.
    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry>;

    /// Get mempool data for the given transaction.
    ///
    /// Returns `None` if the transaction is not in the mempool.
    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry>;
}

impl BitcoinInterface for d::BitcoinD {
    fn is_bitcoind(&self) -> bool {
        true
    }

    fn genesis_block_timestamp(&self) -> u32 {
        self.get_block_stats(
            self.get_block_hash(0)
                .expect("Genesis block hash must always be there"),
        )
        .expect("Genesis block must always be there")
        .time
    }

    fn genesis_block(&self) -> BlockChainTip {
        let height = 0;
        let hash = self
            .get_block_hash(height)
            .expect("Genesis block hash must always be there");
        BlockChainTip { hash, height }
    }

    fn sync_progress(&self) -> SyncProgress {
        self.sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.get_block_hash(tip.height)
            .map(|bh| bh == tip.hash)
            .unwrap_or(false)
    }

    // The watchonly wallet handles this for us.
    fn sync_wallet(
        &mut self,
        _receive_index: ChildNumber,
        _change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        Ok(None)
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathCoincubeDesc],
    ) -> Vec<UTxO> {
        let lsb_res = self.list_since_block(&tip.hash);

        lsb_res
            .received_coins
            .into_iter()
            .filter_map(|entry| {
                let LSBlockEntry {
                    outpoint,
                    amount,
                    block_height,
                    address,
                    parent_descs,
                    is_immature,
                } = entry;
                if parent_descs
                    .iter()
                    .any(|parent_desc| descs.iter().any(|desc| desc == parent_desc))
                {
                    Some(UTxO {
                        outpoint,
                        amount,
                        block_height,
                        address: UTxOAddress::Address(address),
                        is_immature,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        // The confirmed and expired coins to be returned.
        let mut confirmed = Vec::with_capacity(outpoints.len());
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);

        for op in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(&op.txid) {
                res
            } else {
                log::error!("Transaction not in wallet for coin '{}'.", op);
                continue;
            };

            // If the transaction was confirmed, mark the coin as such.
            if let Some(block) = res.block {
                // Do not mark immature coinbase deposits as confirmed until they become mature.
                if res.is_coinbase && res.confirmations < COINBASE_MATURITY {
                    log::debug!("Coin at '{}' comes from an immature coinbase transaction with {} confirmations. Not marking it as confirmed for now.", op, res.confirmations);
                    continue;
                }
                confirmed.push((*op, block.height, block.time));
                continue;
            }

            // If the transaction was dropped from the mempool, discard the coin.
            if !self.is_in_mempool(&op.txid) {
                expired.push(*op);
            }
        }

        (confirmed, expired)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        let mut spent = Vec::with_capacity(outpoints.len());

        for op in outpoints {
            if self.is_spent(op) {
                let spending_txid = if let Some(txid) = self.get_spender_txid(op) {
                    txid
                } else {
                    // TODO: better handling of this edge case.
                    log::error!(
                        "Could not get spender of '{}'. Not reporting it as spending.",
                        op
                    );
                    continue;
                };

                spent.push((*op, spending_txid));
            }
        }

        spent
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        // Spend coins to be returned.
        let mut spent = Vec::with_capacity(outpoints.len());
        // Coins whose spending transaction isn't in our local mempool anymore.
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);

        for (op, txid) in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(txid) {
                res
            } else {
                log::error!("Could not get tx {} spending coin {}.", txid, op);
                continue;
            };

            // If the transaction was confirmed, mark it as such.
            if let Some(block) = res.block {
                spent.push((*op, *txid, block.height, block.time));
                continue;
            }

            // If a conflicting transaction was confirmed instead, replace the txid of the
            // spender for this coin with it and mark it as confirmed.
            let conflict = res.conflicting_txs.iter().find_map(|txid| {
                tx_getter.get_transaction(txid).and_then(|tx| {
                    tx.block.and_then(|block| {
                        // Being part of our watchonly wallet isn't enough, as it could be a
                        // conflicting transaction which spends a different set of coins. Make sure
                        // it does actually spend this coin.
                        tx.tx.input.iter().find_map(|txin| {
                            if &txin.previous_output == op {
                                Some((*txid, block))
                            } else {
                                None
                            }
                        })
                    })
                })
            });
            if let Some((txid, block)) = conflict {
                spent.push((*op, txid, block.height, block.time));
                continue;
            }

            // If the transaction was not confirmed, a conflicting transaction spending this coin
            // too wasn't mined, but still isn't in our mempool anymore, mark the spend as expired.
            if !self.is_in_mempool(txid) {
                expired.push(*op);
            }
        }

        (spent, expired)
    }

    fn common_ancestor(&self, tip: &BlockChainTip, max_depth: i32) -> AncestorSearch {
        let Some(mut stats) = self.get_block_stats(tip.hash) else {
            return AncestorSearch::Failed;
        };
        let mut ancestor = *tip;

        let mut steps = 0;
        while stats.confirmations == -1 {
            if steps >= max_depth {
                return AncestorSearch::TooDeep;
            }
            steps += 1;
            let Some(previous) = stats.previous_blockhash else {
                return AncestorSearch::Failed;
            };
            let Some(next) = self.get_block_stats(previous) else {
                return AncestorSearch::Failed;
            };
            stats = next;
            ancestor = BlockChainTip {
                hash: stats.blockhash,
                height: stats.height,
            };
        }

        AncestorSearch::Found(ancestor)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        match self.broadcast_tx(tx) {
            Ok(()) => Ok(()),
            Err(BitcoindError::Server(e)) => Err(e.to_string()),
            // We assume the Bitcoin backend doesn't fail, so it must be a JSONRPC error.
            Err(e) => panic!(
                "Unexpected Bitcoin error when broadcasting transaction: {}",
                e
            ),
        }
    }

    fn start_rescan(
        &mut self,
        desc: &descriptors::CoincubeDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        // FIXME: in theory i think this could potentially fail to actually start the rescan.
        self.start_rescan(desc, timestamp)
            .map_err(|e| e.to_string())
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.tip_before_timestamp(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        let tip = self.chain_tip();
        Some(self.get_block_stats(tip.hash)?.time)
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.get_transaction(txid).map(|res| (res.tx, res.block))
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.mempool_txs_spending_prevouts(outpoints)
            .into_iter()
            .filter_map(|txid| self.mempool_entry(&txid))
            .collect()
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.mempool_entry(txid)
    }
}

// Shared coin-projection helpers used by BOTH the Electrum and Esplora
// `BitcoinInterface` impls (CQ-DESK-002). Both backends return the same
// `HashMap<OutPoint, Coin>` from `wallet_coins`, and previously duplicated these
// four bodies byte-for-byte. Keeping the projection in one place stops the two
// backends from silently diverging.

fn received_coins_from(coins: &HashMap<bitcoin::OutPoint, Coin>, tip: &BlockChainTip) -> Vec<UTxO> {
    // Wallet coins that are either unconfirmed or confirmed after `tip`. The
    // poller discards any that had already been received.
    coins
        .values()
        .filter_map(|c| {
            let height = c.block_info.map(|info| info.height);
            if height.filter(|h| *h <= tip.height).is_some() {
                None
            } else {
                Some(UTxO {
                    outpoint: c.outpoint,
                    block_height: height,
                    amount: c.amount,
                    address: UTxOAddress::DerivIndex(c.derivation_index, c.is_change),
                    is_immature: c.is_immature,
                })
            }
        })
        .collect()
}

fn confirmed_coins_from(
    coins: &HashMap<bitcoin::OutPoint, Coin>,
    outpoints: &[bitcoin::OutPoint],
) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
    let mut confirmed = Vec::new();
    let mut expired = Vec::new();
    for op in outpoints {
        if let Some(w_c) = coins.get(op) {
            if let Some(block) = w_c.block_info {
                if w_c.is_immature {
                    log::debug!(
                        "Coin at '{}' comes from an immature coinbase transaction at \
                        block height {}. Not marking it as confirmed for now.",
                        op,
                        block.height
                    );
                    continue;
                }
                confirmed.push((w_c.outpoint, block.height, block.time));
            }
        } else {
            expired.push(*op);
        }
    }
    (confirmed, expired)
}

fn spending_coins_from(
    coins: &HashMap<bitcoin::OutPoint, Coin>,
    outpoints: &[bitcoin::OutPoint],
) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
    outpoints
        .iter()
        .filter_map(|op| {
            if let Some(w_c) = coins.get(op) {
                w_c.spend_txid.map(|txid| (w_c.outpoint, txid))
            } else {
                None
            }
        })
        .collect()
}

fn spent_coins_from(
    coins: &HashMap<bitcoin::OutPoint, Coin>,
    outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
    let mut spent = Vec::new();
    let mut expired_spending = Vec::new();
    for (op, spend_txid) in outpoints {
        if let Some(w_c) = coins.get(op) {
            if w_c.spend_txid != Some(*spend_txid) {
                expired_spending.push(*op);
            }
            // Record the coin's *current* spend txid rather than the requested
            // one: after an RBF replacement `w_c.spend_txid` differs from
            // `spend_txid`, and persisting the stale requested txid would leave
            // the DB pointing at a spend that no longer matches the confirmed
            // transaction. (`spend_txid` is always set when `spend_block` is.)
            if let (Some(current_txid), Some(block)) = (w_c.spend_txid, w_c.spend_block) {
                spent.push((*op, current_txid, block.height, block.time));
            }
        }
    }
    (spent, expired_spending)
}

impl BitcoinInterface for electrum::Electrum {
    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        self.sync_wallet(receive_index, change_index)
            .map_err(|e| e.to_string())
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        _descs: &[descriptors::SinglePathCoincubeDesc],
    ) -> Vec<UTxO> {
        received_coins_from(&self.wallet_coins(None), tip)
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        confirmed_coins_from(&self.wallet_coins(Some(outpoints)), outpoints)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        spending_coins_from(&self.wallet_coins(Some(outpoints)), outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        let ops: Vec<_> = outpoints.iter().map(|(op, _)| op).copied().collect();
        spent_coins_from(&self.wallet_coins(Some(&ops)), outpoints)
    }

    fn genesis_block_timestamp(&self) -> u32 {
        self.client()
            .genesis_block_timestamp()
            .expect("Genesis block timestamp must always be there")
    }

    fn genesis_block(&self) -> BlockChainTip {
        self.client()
            .genesis_block()
            .expect("Genesis block must always be there")
    }

    fn chain_tip(&self) -> BlockChainTip {
        // We want the wallet's local chain tip after syncing.
        self.wallet_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        // Return `false` if no block at same height as `tip`
        // is in wallet's local chain.
        self.is_in_wallet_chain(*tip).unwrap_or_default()
    }

    fn walks_common_ancestor(&self) -> bool {
        false
    }

    /// The common ancestor is returned by `sync_wallet()`; this backend keeps no view
    /// of our chain to walk back through, so it cannot answer. The poller checks
    /// [`BitcoinInterface::walks_common_ancestor`] and never calls this — answering
    /// `Failed` rather than panicking is so that a caller which forgets to check
    /// degrades to a skipped poll instead of killing the process.
    ///
    /// FIXME: make the Bitcoin backend interface higher level. See the comment in the poller next
    /// to the `sync_wallet()` call.
    fn common_ancestor(&self, _tip: &BlockChainTip, _max_depth: i32) -> AncestorSearch {
        AncestorSearch::Failed
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        match self.client().broadcast_tx(tx) {
            Ok(_txid) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.wallet_transaction(txid)
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.client().mempool_entry(txid).ok()?
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.client()
            .mempool_spenders(outpoints)
            .unwrap_or_default()
    }

    fn sync_progress(&self) -> SyncProgress {
        // Always return 100% for now since the API is bitcoind-specific to mean "blocks/headers".
        // But in the future it would be nice to inform the user about the progress of the sync
        // if it takes a few dozen seconds.
        let blocks = self.chain_tip().height as u64;
        SyncProgress::new(1.0, blocks, blocks)
    }

    fn start_rescan(
        &mut self,
        _desc: &descriptors::CoincubeDescriptor,
        _timestamp: u32,
    ) -> Result<(), String> {
        self.trigger_rescan();
        Ok(())
    }

    fn rescan_progress(&self) -> Option<f64> {
        // Until we sync we're at 0%. After the sync, we're at 100%.
        self.is_rescanning().then_some(0.0)
    }

    fn block_before_date(&self, _timestamp: u32) -> Option<BlockChainTip> {
        Some(self.genesis_block())
    }

    fn tip_time(&self) -> Option<u32> {
        self.client().tip_time().ok()
    }
}

impl BitcoinInterface for esplora::Esplora {
    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        self.sync_wallet(receive_index, change_index)
            .map_err(|e| e.to_string())
    }

    fn request_eager_sync(&mut self) {
        self.request_eager_sync();
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        _descs: &[descriptors::SinglePathCoincubeDesc],
    ) -> Vec<UTxO> {
        received_coins_from(&self.wallet_coins(None), tip)
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        confirmed_coins_from(&self.wallet_coins(Some(outpoints)), outpoints)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        spending_coins_from(&self.wallet_coins(Some(outpoints)), outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        let ops: Vec<_> = outpoints.iter().map(|(op, _)| op).copied().collect();
        spent_coins_from(&self.wallet_coins(Some(&ops)), outpoints)
    }

    fn genesis_block_timestamp(&self) -> u32 {
        self.client().genesis_block_timestamp().unwrap_or(0)
    }

    fn genesis_block(&self) -> BlockChainTip {
        let hash = self
            .client()
            .genesis_block_hash()
            .expect("Genesis block hash must always be there");
        BlockChainTip { hash, height: 0 }
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.wallet_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.is_in_wallet_chain(*tip).unwrap_or_default()
    }

    fn walks_common_ancestor(&self) -> bool {
        false
    }

    /// The common ancestor is returned by `sync_wallet()`; this backend keeps no view
    /// of our chain to walk back through, so it cannot answer. The poller checks
    /// [`BitcoinInterface::walks_common_ancestor`] and never calls this — answering
    /// `Failed` rather than panicking is so that a caller which forgets to check
    /// degrades to a skipped poll instead of killing the process.
    ///
    /// FIXME: make the Bitcoin backend interface higher level. See the comment in the poller next
    /// to the `sync_wallet()` call.
    fn common_ancestor(&self, _tip: &BlockChainTip, _max_depth: i32) -> AncestorSearch {
        AncestorSearch::Failed
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        self.client().broadcast_tx(tx).map_err(|e| e.to_string())
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.wallet_transaction(txid)
    }

    fn mempool_entry(&self, _txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        // Esplora API doesn't expose mempool fee aggregation; return None.
        None
    }

    fn mempool_spenders(&self, _outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        // Esplora API doesn't expose mempool spender fee data; return empty.
        Vec::new()
    }

    fn sync_progress(&self) -> SyncProgress {
        let blocks = self.chain_tip().height as u64;
        SyncProgress::new(1.0, blocks, blocks)
    }

    fn start_rescan(
        &mut self,
        _desc: &descriptors::CoincubeDescriptor,
        _timestamp: u32,
    ) -> Result<(), String> {
        self.trigger_rescan();
        Ok(())
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.is_rescanning().then_some(0.0)
    }

    fn block_before_date(&self, _timestamp: u32) -> Option<BlockChainTip> {
        Some(self.genesis_block())
    }

    fn tip_time(&self) -> Option<u32> {
        self.client().tip_time().ok()
    }
}

// FIXME: do we need to repeat the entire trait implementation? Isn't there a nicer way?
impl BitcoinInterface for sync::Arc<sync::Mutex<dyn BitcoinInterface + 'static>> {
    fn is_bitcoind(&self) -> bool {
        self.lock().unwrap().is_bitcoind()
    }

    fn genesis_block_timestamp(&self) -> u32 {
        self.lock().unwrap().genesis_block_timestamp()
    }

    fn genesis_block(&self) -> BlockChainTip {
        self.lock().unwrap().genesis_block()
    }

    fn sync_progress(&self) -> SyncProgress {
        self.lock().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.lock().unwrap().chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.lock().unwrap().is_in_chain(tip)
    }

    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        self.lock()
            .unwrap()
            .sync_wallet(receive_index, change_index)
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathCoincubeDesc],
    ) -> Vec<UTxO> {
        self.lock().unwrap().received_coins(tip, descs)
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        self.lock().unwrap().confirmed_coins(outpoints)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        self.lock().unwrap().spending_coins(outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        self.lock().unwrap().spent_coins(outpoints)
    }

    fn common_ancestor(&self, tip: &BlockChainTip, max_depth: i32) -> AncestorSearch {
        self.lock().unwrap().common_ancestor(tip, max_depth)
    }

    fn walks_common_ancestor(&self) -> bool {
        self.lock().unwrap().walks_common_ancestor()
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        self.lock().unwrap().broadcast_tx(tx)
    }

    fn request_eager_sync(&mut self) {
        self.lock().unwrap().request_eager_sync();
    }

    fn start_rescan(
        &mut self,
        desc: &descriptors::CoincubeDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        self.lock().unwrap().start_rescan(desc, timestamp)
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.lock().unwrap().rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.lock().unwrap().block_before_date(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        self.lock().unwrap().tip_time()
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.lock().unwrap().wallet_transaction(txid)
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.lock().unwrap().mempool_spenders(outpoints)
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.lock().unwrap().mempool_entry(txid)
    }
}

// FIXME: We could avoid this type (and all the conversions entailing allocations) if bitcoind
// exposed the derivation index from the parent descriptor in the LSB result.
#[derive(Debug, Clone)]
pub struct UTxO {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub block_height: Option<i32>,
    pub address: UTxOAddress,
    pub is_immature: bool,
}

/// Details about the UTXO address.
#[derive(Debug, Clone)]
pub enum UTxOAddress {
    Address(bitcoin::Address<address::NetworkUnchecked>),
    /// Derivation index and whether it is from the change descriptor.
    DerivIndex(ChildNumber, bool),
}

#[derive(Debug, Clone, Copy)]
pub struct BlockInfo {
    pub height: i32,
    pub time: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Coin {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub derivation_index: ChildNumber,
    pub is_change: bool,
    pub is_immature: bool,
    pub block_info: Option<BlockInfo>,
    pub spend_txid: Option<bitcoin::Txid>,
    pub spend_block: Option<BlockInfo>,
}
