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

use std::{collections::HashMap, fmt, net, sync};

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

/// A failed backend lookup is not an invariant failure of the chain itself.
/// Keep the original error typed so startup can surface admission refusals.
#[derive(Debug)]
pub enum GenesisError {
    Bitcoind(Box<BitcoindError>),
    Electrum(Box<electrum::client::Error>),
    Esplora(Box<esplora::client::Error>),
}

impl fmt::Display for GenesisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bitcoind(error) => write!(f, "Genesis lookup failed: {}", error),
            Self::Electrum(error) => write!(f, "Genesis lookup failed: {}", error),
            Self::Esplora(error) => write!(f, "Genesis lookup failed: {}", error),
        }
    }
}

impl std::error::Error for GenesisError {}

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

/// The poller could not complete a poll. Kept until the next poll that does.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PollFailure {
    /// The latest failure's message.
    pub message: String,
    /// When the current run of consecutive failures began.
    pub since: u32,
    /// When the latest failure happened.
    pub last_at: u32,
    /// Polls that have failed in a row, the latest included.
    pub consecutive: u32,
}

/// A snapshot of how far the wallet's view of its own history has got, as reported by
/// `get_info`. Absent from older daemons, so every field defaults.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HistorySync {
    /// A full scan of the wallet's addresses is under way. On Electrum and Esplora
    /// this is how a wallet learns its history; until it finishes, the database
    /// holds none of the transactions it is looking for.
    pub full_scan_in_progress: bool,
    /// Addresses the running full scan has looked up so far.
    pub addresses_checked: u64,
    /// Addresses the running full scan is expected to look up. An estimate: the scan
    /// stops after a gap of unused addresses, which it can only know by looking.
    pub addresses_expected: u64,
    /// When a full scan last finished and its result reached the database, during this
    /// run of the daemon. `None` until then, including while a first scan runs.
    pub full_scan_completed_at: Option<u32>,
    /// Monotonic within this daemon; distinguishes same-second scan retries.
    #[serde(default)]
    pub full_scan_generation: u64,
    /// Generation whose result was successfully committed by the poller.
    #[serde(default)]
    pub full_scan_completed_generation: u64,
    /// The poller's current run of failed polls, if the latest poll failed.
    pub last_failure: Option<PollFailure>,
}

impl HistorySync {
    /// Progress of the running full scan, between 0 and 1. Capped below 1, since
    /// `addresses_expected` is an estimate and only the scan finishing means it is done.
    pub fn full_scan_progress(&self) -> Option<f64> {
        if !self.full_scan_in_progress {
            return None;
        }
        if self.addresses_expected == 0 {
            return Some(0.0);
        }
        Some((self.addresses_checked as f64 / self.addresses_expected as f64).min(0.99))
    }
}

/// Live record of the wallet's history synchronisation, read by `get_info` WITHOUT
/// taking the `BitcoinInterface` mutex, for the same reason as [`SyncProgressCache`]:
/// the poller holds that mutex for an entire full scan, which over a slow Esplora
/// server can take most of an hour.
///
/// The backend publishes full-scan progress into it as each address is looked up; the
/// poller publishes the outcome of every poll. A full scan only counts as completed once
/// the poll carrying it succeeds — that is when its transactions reach the database.
#[derive(Debug, Default)]
pub struct HistorySyncCache {
    // User-requested eager polls must be recorded without waiting for the
    // backend mutex held by a long historical scan. The poller consumes this
    // immediately before its next poll, even if its wake-up channel was full.
    eager_sync_requested: sync::atomic::AtomicBool,
    scanning: sync::atomic::AtomicBool,
    checked: sync::atomic::AtomicU64,
    receive_checked: sync::atomic::AtomicU64,
    change_checked: sync::atomic::AtomicU64,
    expected: sync::atomic::AtomicU64,
    /// A full scan finished in the backend; the poll that writes it to the database
    /// has not succeeded yet.
    awaiting_commit: sync::atomic::AtomicBool,
    /// Unix time of the last committed full scan; 0 for none.
    completed_at: sync::atomic::AtomicU32,
    generation: sync::atomic::AtomicU64,
    completed_generation: sync::atomic::AtomicU64,
    fetched_generation: sync::atomic::AtomicU64,
    failure: sync::Mutex<Option<PollFailure>>,
}

impl HistorySyncCache {
    pub fn request_eager_sync(&self) {
        self.eager_sync_requested
            .store(true, sync::atomic::Ordering::Release);
    }

    pub fn take_eager_sync_request(&self) -> bool {
        self.eager_sync_requested
            .swap(false, sync::atomic::Ordering::AcqRel)
    }

    /// A full scan expected to look up `expected` addresses is starting. Restarts the
    /// count, so a scan that is retried against another server counts from zero.
    pub fn begin_full_scan(&self, expected: u64) {
        use sync::atomic::Ordering::{Relaxed, Release};
        self.generation.fetch_add(1, Relaxed);
        self.checked.store(0, Relaxed);
        self.receive_checked.store(0, Relaxed);
        self.change_checked.store(0, Relaxed);
        self.expected.store(expected, Relaxed);
        // Release, paired with the Acquire in `snapshot`: a reader that sees this
        // scan running also sees its reset count and estimate, never the previous
        // scan's.
        self.scanning.store(true, Release);
    }

    /// The running full scan looked up one more address.
    pub fn address_checked(&self) {
        self.checked.fetch_add(1, sync::atomic::Ordering::Relaxed);
    }

    /// Count an actually queried address on its branch. The generic checked
    /// count still drives the existing progress indicator.
    pub fn branch_address_checked(&self, change: bool) {
        self.address_checked();
        let counter = if change {
            &self.change_checked
        } else {
            &self.receive_checked
        };
        counter.fetch_add(1, sync::atomic::Ordering::Relaxed);
    }

    /// A backend returned a complete full scan, but the poller has not yet
    /// persisted its wallet update. Counts remain pending on a failed poll.
    pub fn pending_full_scan_coverage(&self) -> Option<(u32, u32)> {
        use sync::atomic::Ordering::{Acquire, Relaxed};
        self.awaiting_commit.load(Acquire).then(|| {
            let bounded =
                |count: &sync::atomic::AtomicU64| count.load(Relaxed).min(i32::MAX as u64) as u32;
            (
                bounded(&self.receive_checked),
                bounded(&self.change_checked),
            )
        })
    }

    /// The full scan returned a result for the backend to apply.
    pub fn full_scan_fetched(&self) {
        use sync::atomic::Ordering::Relaxed;
        self.fetched_generation.store(
            self.generation.load(sync::atomic::Ordering::Acquire),
            Relaxed,
        );
        self.scanning.store(false, Relaxed);
        self.awaiting_commit
            .store(true, sync::atomic::Ordering::Release);
    }

    /// A poll completed: its result, including any full scan it carried, is in the
    /// database. Clears the failure run.
    pub fn poll_succeeded(&self, now: u32) {
        use sync::atomic::Ordering::Relaxed;
        self.scanning.store(false, Relaxed);
        if self.awaiting_commit.swap(false, Relaxed) {
            self.completed_at.store(now, Relaxed);
            self.completed_generation.store(
                self.fetched_generation
                    .load(sync::atomic::Ordering::Acquire),
                sync::atomic::Ordering::Release,
            );
        }
        *self.failure.lock().expect("never held across a panic") = None;
    }

    /// A poll failed with `message`. Any full scan it was running is lost — the next
    /// one starts over — but a scan that already returned stays awaiting its commit:
    /// the backend applied it and the next successful poll writes it out.
    ///
    /// Returns the updated failure run and whether `message` differs from the previous
    /// failure's, so the caller can log a new problem loudly and a repeat quietly.
    pub fn poll_failed(&self, message: &str, now: u32) -> (PollFailure, bool) {
        self.scanning.store(false, sync::atomic::Ordering::Relaxed);
        let mut failure = self.failure.lock().expect("never held across a panic");
        let (updated, changed) = match failure.take() {
            Some(previous) => {
                let changed = error_kind(&previous.message) != error_kind(message);
                (
                    PollFailure {
                        message: message.to_string(),
                        since: previous.since,
                        last_at: now,
                        consecutive: previous.consecutive.saturating_add(1),
                    },
                    changed,
                )
            }
            None => (
                PollFailure {
                    message: message.to_string(),
                    since: now,
                    last_at: now,
                    consecutive: 1,
                },
                true,
            ),
        };
        *failure = Some(updated.clone());
        (updated, changed)
    }

    /// The current state, for `get_info`.
    pub fn snapshot(&self) -> HistorySync {
        use sync::atomic::Ordering::{Acquire, Relaxed};
        let full_scan_in_progress = self.scanning.load(Acquire);
        HistorySync {
            full_scan_in_progress,
            full_scan_generation: self.generation.load(Acquire),
            full_scan_completed_generation: self.completed_generation.load(Acquire),
            addresses_checked: if full_scan_in_progress {
                self.checked.load(Relaxed)
            } else {
                0
            },
            addresses_expected: if full_scan_in_progress {
                self.expected.load(Relaxed)
            } else {
                0
            },
            full_scan_completed_at: match self.completed_at.load(Relaxed) {
                0 => None,
                at => Some(at),
            },
            last_failure: self
                .failure
                .lock()
                .expect("never held across a panic")
                .clone(),
        }
    }
}

/// A failure message with its long numbers removed, so request ids, timestamps and
/// byte counts in a server's error text do not make every repeat of the same failure
/// look like a new one. Runs of fewer than four digits are kept: an HTTP status is
/// the difference between a rate limit (429) and an outage (503).
fn error_kind(message: &str) -> String {
    const MIN_VOLATILE_DIGITS: usize = 4;
    let mut kind = String::with_capacity(message.len());
    let mut digits = String::new();
    for c in message.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if digits.len() < MIN_VOLATILE_DIGITS {
            kind.push_str(&digits);
        }
        digits.clear();
        if c != '\0' {
            kind.push(c);
        }
    }
    kind
}

/// How many addresses a full scan with this `stop_gap` is expected to look up: each
/// keychain's revealed addresses plus a gap's worth past them. `last_revealed` holds the
/// last revealed index of each keychain that has one.
pub(crate) fn expected_full_scan_addresses(
    keychains: usize,
    last_revealed: impl IntoIterator<Item = u32>,
    stop_gap: usize,
) -> u64 {
    let revealed: u64 = last_revealed.into_iter().map(|i| u64::from(i) + 1).sum();
    revealed + (keychains as u64) * (stop_gap as u64)
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

/// Which `bitcoind` a backend talks to.
///
/// The socket alone is a *location*, not an identity: a different node, or the same
/// node rebuilt on a different datadir, can later occupy the same address. So the
/// credential descriptor comes along with it — for the managed node that is the path
/// to a cookie file living inside the node's own datadir, which moves when the
/// datadir does. Fingerprinted rather than carried verbatim, because this ends up
/// written to a state file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendId {
    /// Where it listens.
    pub addr: net::SocketAddr,
    /// Hex SHA-256 of the credential descriptor — the cookie file's path, or the
    /// RPC username. Never the password.
    pub credentials: String,
}

impl BackendId {
    /// Fingerprint a credential descriptor. Hashed so that neither a filesystem
    /// layout nor a username is written to disk verbatim.
    pub fn fingerprint(descriptor: &str) -> String {
        use miniscript::bitcoin::hashes::{sha256, Hash};
        sha256::Hash::hash(descriptor.as_bytes()).to_string()
    }
}

/// Confirmed coins and unconfirmed coins that expired during a poll.
pub type ConfirmedCoins = (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>);

/// Our Bitcoin backend.
/// Tickets prevent an older successful poll from acknowledging a newer request.
#[derive(Debug)]
pub(crate) struct WalletRecordReplay {
    requested: u64,
    acknowledged: u64,
}
impl Default for WalletRecordReplay {
    fn default() -> Self {
        Self {
            requested: 1,
            acknowledged: 0,
        }
    }
}
impl WalletRecordReplay {
    pub(crate) fn request(&mut self) -> Result<u64, String> {
        self.requested = self
            .requested
            .checked_add(1)
            .ok_or("Wallet replay ticket exhausted")?;
        Ok(self.requested)
    }
    pub(crate) fn pending(&self) -> Option<u64> {
        (self.requested > self.acknowledged).then_some(self.requested)
    }
    pub(crate) fn acknowledge(&mut self, ticket: u64) {
        self.acknowledged = self.acknowledged.max(ticket.min(self.requested));
    }
}

pub trait BitcoinInterface: Send {
    fn request_wallet_record_replay(&mut self) -> Result<u64, String> {
        Err("Wallet record replay requires a regular Bitcoin Core/Knots backend".into())
    }
    fn wallet_record_replay_pending(&self) -> Option<u64> {
        None
    }
    /// Explicit recovery must not claim completion with unmapped owned outputs.
    /// The automatic startup replay is best effort so outliers cannot stop sync.
    fn wallet_record_replay_requires_complete_mapping(&self) -> bool {
        self.wallet_record_replay_pending()
            .is_some_and(|ticket| ticket > 1)
    }
    fn acknowledge_wallet_record_replay(&mut self, _ticket: u64) {}
    /// Install the daemon shutdown signal for bounded polling reads.
    fn set_poll_abort(&mut self, _abort: sync::Arc<sync::atomic::AtomicBool>) {}

    /// Install the record a full scan reports its progress into. Only backends that
    /// learn a wallet's history by scanning its addresses (Electrum, Esplora) use it.
    fn set_history_sync_cache(&mut self, _cache: sync::Arc<HistorySyncCache>) {}

    /// Whether this backend talks to a `bitcoind`.
    ///
    /// Used to scope the managed-node maintenance pause: an Esplora or Electrum
    /// Vault is unaffected by a rewind of the managed node and must keep polling.
    /// An external `bitcoind` we cannot distinguish from the managed one, so it
    /// pauses too — conservative, and bounded by the rewind's duration.
    fn is_bitcoind(&self) -> bool {
        false
    }

    /// Which `bitcoind` this backend talks to, when it talks to one at all.
    ///
    /// Used to scope a sanctioned rollback to the node a repair was actually
    /// performed on: the blocks a repair names are public, so a chain-level check
    /// cannot tell the managed node from any other node following the same chain.
    fn backend_id(&self) -> Option<BackendId> {
        None
    }

    fn genesis_block_timestamp(&self) -> Result<u32, GenesisError>;

    fn genesis_block(&self) -> Result<BlockChainTip, GenesisError>;

    /// Get the progress of the block chain synchronization.
    /// Returns a rounded up percentage between 0 and 1. Use the `is_synced` method to be sure the
    /// backend is completely synced to the best known tip.
    fn sync_progress(&self) -> SyncProgress;

    /// Fallible polling read; unavailable data must not replace cached progress.
    fn try_sync_progress(&self) -> Result<SyncProgress, String> {
        Ok(self.sync_progress())
    }

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Fallible polling read; an outage is not a different or missing chain tip.
    fn try_chain_tip(&self) -> Result<BlockChainTip, String> {
        Ok(self.chain_tip())
    }

    /// Fallible read of the best block for RPC commands (#589). Unlike
    /// [`Self::try_chain_tip`] it may use the backend's interactive retry budget;
    /// unlike [`Self::chain_tip`] an outage outlasting it is an error, not a panic.
    fn chain_tip_result(&self) -> Result<BlockChainTip, String> {
        Ok(self.chain_tip())
    }

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
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
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

    fn try_received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        Ok(self.received_coins(tip, descs))
    }

    /// Return every wallet output discovered by a historical imported-wallet scan,
    /// including outputs confirmed before the database's current chain tip. The
    /// poller de-duplicates these against its durable outpoints. Other backends
    /// retain their ordinary tip-bounded behavior unless they support this scan.
    fn try_historical_received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        self.try_received_coins(tip, descs)
    }
    fn try_confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<ConfirmedCoins, String> {
        Ok(self.confirmed_coins(outpoints))
    }
    fn try_spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<(bitcoin::OutPoint, bitcoin::Txid)>, String> {
        Ok(self.spending_coins(outpoints))
    }
    fn try_spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Result<(Vec<SpentCoin>, Vec<bitcoin::OutPoint>), String> {
        Ok(self.spent_coins(outpoints))
    }
    fn try_is_in_chain(&self, tip: &BlockChainTip) -> Result<bool, String> {
        Ok(self.is_in_chain(tip))
    }
    fn try_rescan_progress(&self) -> Result<Option<f64>, String> {
        Ok(self.rescan_progress())
    }
    fn try_block_before_date(&self, timestamp: u32) -> Result<Option<BlockChainTip>, String> {
        Ok(self.block_before_date(timestamp))
    }
    fn try_wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Result<Option<(bitcoin::Transaction, Option<Block>)>, String> {
        Ok(self.wallet_transaction(txid))
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

    /// Fallible [`Self::rescan_progress`] for RPC commands (#589): `Ok(None)` is
    /// the backend saying no rescan runs; an unreachable backend is an `Err`.
    fn rescan_progress_result(&self) -> Result<Option<f64>, String> {
        Ok(self.rescan_progress())
    }

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

    /// Fallible [`Self::mempool_spenders`] for RPC commands (#594): an empty list
    /// is the backend saying nothing spends these outpoints; an unreachable
    /// backend, or an answer that cannot be used, is an `Err`.
    ///
    /// bitcoind and Electrum override this (#597). The default wraps the
    /// infallible read and so can never return `Err`: a backend that keeps it
    /// (Esplora, which has no mempool reads and always answers empty) cannot
    /// tell an outage apart from "nothing spends these outpoints".
    fn mempool_spenders_result(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<MempoolEntry>, String> {
        Ok(self.mempool_spenders(outpoints))
    }

    /// Fallible [`Self::mempool_entry`] for RPC commands (#594): `Ok(None)` is the
    /// backend saying the transaction is not in its mempool; an unreachable
    /// backend, or an answer that cannot be used, is an `Err`.
    ///
    /// bitcoind and Electrum override this (#597). The default wraps the
    /// infallible read and so can never return `Err`: a backend that keeps it
    /// (Esplora, which has no mempool reads and always answers `None`) cannot
    /// tell an outage apart from "not in the mempool".
    fn mempool_entry_result(&self, txid: &bitcoin::Txid) -> Result<Option<MempoolEntry>, String> {
        Ok(self.mempool_entry(txid))
    }

    /// Whether [`Self::mempool_spenders_result`] reads this backend's mempool.
    ///
    /// A backend that answers `false` (Esplora) always reports an empty mempool,
    /// which reads exactly like "nothing spends these outpoints". `rbfpsbt` takes
    /// that answer as what a replacement must outbid, so it must not trust it
    /// here: it would price every cancel at the 1 sat/vb relay minimum, below the
    /// transaction being replaced, and the network would refuse the replacement.
    fn sees_mempool(&self) -> bool {
        true
    }
}

/// Find a confirmed conflicting spender without treating assumed confirmation as final.
fn confirmed_conflicting_spender(
    getter: &mut CachedTxGetter<'_>,
    conflicts: Vec<bitcoin::Txid>,
    op: &bitcoin::OutPoint,
) -> Result<(Option<(bitcoin::Txid, Block)>, bool), String> {
    let mut confirmed_conflict = None;
    let mut assumed_conflict = false;
    for conflict in conflicts {
        if let Some(other) = getter.try_get_transaction(&conflict)? {
            if other.has_assumed_confirmation() {
                assumed_conflict |= other.tx.input.iter().any(|i| i.previous_output == *op);
                continue;
            }
            if let Some(block) = other.block {
                if other
                    .tx
                    .input
                    .iter()
                    .any(|input| input.previous_output == *op)
                {
                    confirmed_conflict = Some((conflict, block));
                    break;
                }
            }
        }
    }
    Ok((confirmed_conflict, assumed_conflict))
}

/// Find a mined conflict that actually spends the requested outpoint.
fn legacy_confirmed_conflicting_spender(
    tx_getter: &mut CachedTxGetter<'_>,
    conflicts: &[bitcoin::Txid],
    op: &bitcoin::OutPoint,
) -> Option<(bitcoin::Txid, Block)> {
    conflicts.iter().find_map(|txid| {
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
    })
}

impl BitcoinInterface for d::BitcoinD {
    fn request_wallet_record_replay(&mut self) -> Result<u64, String> {
        if self.local_fork_chain.is_some() {
            return Err("Wallet record replay is available for regular Bitcoin only".into());
        }
        self.record_replay.request()
    }
    fn wallet_record_replay_pending(&self) -> Option<u64> {
        if self.local_fork_chain.is_some() {
            None
        } else {
            self.record_replay.pending()
        }
    }
    fn acknowledge_wallet_record_replay(&mut self, ticket: u64) {
        self.record_replay.acknowledge(ticket);
    }
    fn set_poll_abort(&mut self, abort: sync::Arc<sync::atomic::AtomicBool>) {
        self.poll_abort = abort;
    }

    fn try_is_in_chain(&self, tip: &BlockChainTip) -> Result<bool, String> {
        Ok(self
            .try_get_block_hash(tip.height)?
            .is_some_and(|hash| hash == tip.hash))
    }

    fn try_received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        // Core keeps recovered transaction records even after their bodies are
        // pruned. Replay those records once on startup and after explicit scans.
        let hash = if self.wallet_record_replay_pending().is_some() {
            self.try_get_block_hash(0)?
                .ok_or("Missing genesis for wallet record replay")?
        } else {
            tip.hash
        };
        Ok(self
            .try_list_since_block(&hash)?
            .received_coins
            .into_iter()
            .filter_map(|entry| {
                if !entry
                    .parent_descs
                    .iter()
                    .any(|parent| descs.iter().any(|desc| desc == parent))
                {
                    return None;
                }
                Some(UTxO {
                    outpoint: entry.outpoint,
                    amount: entry.amount,
                    block_height: entry.block_height,
                    address: UTxOAddress::Address(entry.address),
                    is_immature: entry.is_immature,
                })
            })
            .collect())
    }

    fn try_confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<ConfirmedCoins, String> {
        let mut confirmed = Vec::new();
        let mut expired = Vec::new();
        let mut getter = CachedTxGetter::new(self);
        for op in outpoints {
            let Some(tx) = getter.try_get_transaction(&op.txid)? else {
                continue;
            };
            if tx.has_assumed_confirmation() {
                continue;
            }
            if let Some(block) = tx.block {
                if !tx.is_coinbase || tx.confirmations >= COINBASE_MATURITY {
                    confirmed.push((*op, block.height, block.time));
                }
            } else if !self.try_is_in_mempool(&op.txid)? {
                expired.push(*op);
            }
        }
        Ok((confirmed, expired))
    }

    fn try_spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<(bitcoin::OutPoint, bitcoin::Txid)>, String> {
        let mut spending = Vec::new();
        for op in outpoints {
            if self.try_is_spent(op)? {
                if let Some(txid) = self.try_get_spender_txid(op)? {
                    spending.push((*op, txid));
                }
            }
        }
        Ok(spending)
    }

    fn try_spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Result<(Vec<SpentCoin>, Vec<bitcoin::OutPoint>), String> {
        let mut spent = Vec::new();
        let mut expired = Vec::new();
        let mut getter = CachedTxGetter::new(self);
        for (op, txid) in outpoints {
            let Some(tx) = getter.try_get_transaction(txid)? else {
                continue;
            };
            if tx.has_assumed_confirmation() {
                continue;
            }
            if let Some(block) = tx.block {
                spent.push((*op, *txid, block.height, block.time));
                continue;
            }
            let (confirmed_conflict, assumed_conflict) =
                confirmed_conflicting_spender(&mut getter, tx.conflicting_txs, op)?;
            if let Some((txid, block)) = confirmed_conflict {
                spent.push((*op, txid, block.height, block.time));
            } else if !assumed_conflict && !self.try_is_in_mempool(txid)? {
                expired.push(*op);
            }
        }
        Ok((spent, expired))
    }

    fn try_wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Result<Option<(bitcoin::Transaction, Option<Block>)>, String> {
        Ok(self.try_get_transaction(txid)?.map(|tx| {
            let block = if tx.has_assumed_confirmation() {
                None
            } else {
                tx.block
            };
            (tx.tx, block)
        }))
    }

    fn try_rescan_progress(&self) -> Result<Option<f64>, String> {
        self.try_rescan_progress()
    }
    fn try_block_before_date(&self, timestamp: u32) -> Result<Option<BlockChainTip>, String> {
        self.try_tip_before_timestamp(timestamp)
    }

    fn is_bitcoind(&self) -> bool {
        true
    }

    fn backend_id(&self) -> Option<BackendId> {
        Some(self.backend_id())
    }

    fn genesis_block_timestamp(&self) -> Result<u32, GenesisError> {
        let hash = self
            .get_block_hash_result(0)
            .map_err(|error| GenesisError::Bitcoind(Box::new(error)))?;
        self.get_block_stats_result(hash)
            .map(|stats| stats.time)
            .map_err(|error| GenesisError::Bitcoind(Box::new(error)))
    }

    fn genesis_block(&self) -> Result<BlockChainTip, GenesisError> {
        let height = 0;
        let hash = self
            .get_block_hash_result(height)
            .map_err(|error| GenesisError::Bitcoind(Box::new(error)))?;
        Ok(BlockChainTip { hash, height })
    }

    fn sync_progress(&self) -> SyncProgress {
        self.sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.chain_tip()
    }

    fn try_chain_tip(&self) -> Result<BlockChainTip, String> {
        self.try_chain_tip()
    }

    fn chain_tip_result(&self) -> Result<BlockChainTip, String> {
        self.chain_tip_result().map_err(|e| e.to_string())
    }

    fn try_sync_progress(&self) -> Result<SyncProgress, String> {
        self.try_sync_progress()
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
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
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
            let conflict =
                legacy_confirmed_conflicting_spender(&mut tx_getter, &res.conflicting_txs, op);
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
        let Ok(mut stats) = self.try_get_block_stats(tip.hash) else {
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
            let Ok(next) = self.try_get_block_stats(previous) else {
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

    fn rescan_progress_result(&self) -> Result<Option<f64>, String> {
        self.rescan_progress_result().map_err(|e| e.to_string())
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.tip_before_timestamp(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        // Only RPC commands ask for this (start_rescan, anti-fee-sniping), so a
        // node that stays down past the retry budget must yield `None` for them
        // to report, not panic the daemon through `chain_tip` (#589).
        let tip = self
            .chain_tip_result()
            .map_err(|e| log::warn!("Cannot read the chain tip for its timestamp: {}", e))
            .ok()?;
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

    fn mempool_spenders_result(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<MempoolEntry>, String> {
        let mut entries = Vec::new();
        for txid in self
            .mempool_txs_spending_prevouts_result(outpoints)
            .map_err(|e| e.to_string())?
        {
            // A spender that left the mempool since the first read is skipped,
            // as in `mempool_spenders`.
            if let Some(entry) = self
                .mempool_entry_result(&txid)
                .map_err(|e| e.to_string())?
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn mempool_entry_result(&self, txid: &bitcoin::Txid) -> Result<Option<MempoolEntry>, String> {
        self.mempool_entry_result(txid).map_err(|e| e.to_string())
    }
}

// Shared coin-projection helpers used by BOTH the Electrum and Esplora
// `BitcoinInterface` impls (CQ-DESK-002). Both backends return the same
// `HashMap<OutPoint, Coin>` from `wallet_coins`, and previously duplicated these
// four bodies byte-for-byte. Keeping the projection in one place stops the two
// backends from silently diverging.

fn received_coins_from(
    coins: &HashMap<bitcoin::OutPoint, Coin>,
    tip: Option<&BlockChainTip>,
) -> Vec<UTxO> {
    // Ordinary polling only offers outputs confirmed after `tip`. Historical
    // import catch-up offers every discovered output, even if its funding block
    // predates the stored tip; the poller de-duplicates by outpoint.
    coins
        .values()
        .filter_map(|c| {
            let height = c.block_info.map(|info| info.height);
            if height.is_some_and(|h| tip.is_some_and(|tip| h <= tip.height)) {
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
        _descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Vec<UTxO> {
        received_coins_from(&self.wallet_coins(None), Some(tip))
    }

    fn try_historical_received_coins(
        &self,
        _tip: &BlockChainTip,
        _descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        Ok(received_coins_from(&self.wallet_coins(None), None))
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

    fn genesis_block_timestamp(&self) -> Result<u32, GenesisError> {
        self.client()
            .genesis_block_timestamp()
            .map_err(|error| GenesisError::Electrum(Box::new(error)))
    }

    fn genesis_block(&self) -> Result<BlockChainTip, GenesisError> {
        self.client()
            .genesis_block()
            .map_err(|error| GenesisError::Electrum(Box::new(error)))
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

    // The client tells a failed exchange apart from absence: a connection that
    // fails past the retries is an `Err`, and so is a hole left in the fee walk
    // by a refused ancestor (`client::Error::IncompleteMempoolGraph`). An unspent
    // outpoint has no spender.
    //
    // Limitation (#597 review): absence is not only "unknown transaction". BDK's
    // `populate_with_txids` skips a requested txid on *any* server error reply
    // (`electrum_client::Error::Protocol`): electrs relaying a failed daemon call
    // ("daemon error: ..."), or a Fulcrum/ElectrumX rate limit, also reads as
    // "not in the mempool". createspend may then use an unconfirmed coin without
    // its ancestors' size and fees and underpay the CPFP package (no funds at
    // risk). rbfpsbt is mostly unaffected: the spender is fetched while syncing
    // the outpoints, where an error reply is an `Err`.
    fn mempool_spenders_result(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<MempoolEntry>, String> {
        self.client()
            .mempool_spenders(outpoints)
            .map_err(|e| e.to_string())
    }

    fn mempool_entry_result(&self, txid: &bitcoin::Txid) -> Result<Option<MempoolEntry>, String> {
        self.client().mempool_entry(txid).map_err(|e| e.to_string())
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

    fn set_history_sync_cache(&mut self, cache: sync::Arc<HistorySyncCache>) {
        self.install_history_sync_cache(cache);
    }

    fn rescan_progress(&self) -> Option<f64> {
        // The share of addresses the running full scan has looked up, until its
        // result is applied. 0% before the scan starts.
        self.is_rescanning()
            .then(|| self.full_scan_progress().unwrap_or(0.0))
    }

    fn block_before_date(&self, _timestamp: u32) -> Option<BlockChainTip> {
        self.genesis_block().ok()
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
        _descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Vec<UTxO> {
        received_coins_from(&self.wallet_coins(None), Some(tip))
    }

    fn try_historical_received_coins(
        &self,
        _tip: &BlockChainTip,
        _descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        Ok(received_coins_from(&self.wallet_coins(None), None))
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

    fn genesis_block_timestamp(&self) -> Result<u32, GenesisError> {
        self.client()
            .genesis_block_timestamp()
            .map_err(|error| GenesisError::Esplora(Box::new(error)))
    }

    fn genesis_block(&self) -> Result<BlockChainTip, GenesisError> {
        let hash = self
            .client()
            .genesis_block_hash()
            .map_err(|error| GenesisError::Esplora(Box::new(error)))?;
        Ok(BlockChainTip { hash, height: 0 })
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

    fn sees_mempool(&self) -> bool {
        false
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

    fn set_history_sync_cache(&mut self, cache: sync::Arc<HistorySyncCache>) {
        self.install_history_sync_cache(cache);
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.is_rescanning()
            .then(|| self.full_scan_progress().unwrap_or(0.0))
    }

    fn block_before_date(&self, _timestamp: u32) -> Option<BlockChainTip> {
        self.genesis_block().ok()
    }

    fn tip_time(&self) -> Option<u32> {
        self.client().tip_time().ok()
    }
}

// FIXME: do we need to repeat the entire trait implementation? Isn't there a nicer way?
impl BitcoinInterface for sync::Arc<sync::Mutex<dyn BitcoinInterface + 'static>> {
    fn request_wallet_record_replay(&mut self) -> Result<u64, String> {
        self.lock().unwrap().request_wallet_record_replay()
    }
    fn wallet_record_replay_pending(&self) -> Option<u64> {
        self.lock().unwrap().wallet_record_replay_pending()
    }
    fn wallet_record_replay_requires_complete_mapping(&self) -> bool {
        self.lock()
            .unwrap()
            .wallet_record_replay_requires_complete_mapping()
    }
    fn acknowledge_wallet_record_replay(&mut self, ticket: u64) {
        self.lock()
            .unwrap()
            .acknowledge_wallet_record_replay(ticket);
    }
    fn try_received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        self.lock().unwrap().try_received_coins(tip, descs)
    }
    fn try_historical_received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        self.lock()
            .unwrap()
            .try_historical_received_coins(tip, descs)
    }
    fn try_confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<ConfirmedCoins, String> {
        self.lock().unwrap().try_confirmed_coins(outpoints)
    }
    fn try_spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<(bitcoin::OutPoint, bitcoin::Txid)>, String> {
        self.lock().unwrap().try_spending_coins(outpoints)
    }
    fn try_spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Result<(Vec<SpentCoin>, Vec<bitcoin::OutPoint>), String> {
        self.lock().unwrap().try_spent_coins(outpoints)
    }
    fn try_is_in_chain(&self, tip: &BlockChainTip) -> Result<bool, String> {
        self.lock().unwrap().try_is_in_chain(tip)
    }
    fn try_rescan_progress(&self) -> Result<Option<f64>, String> {
        self.lock().unwrap().try_rescan_progress()
    }
    fn try_block_before_date(&self, timestamp: u32) -> Result<Option<BlockChainTip>, String> {
        self.lock().unwrap().try_block_before_date(timestamp)
    }
    fn try_wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Result<Option<(bitcoin::Transaction, Option<Block>)>, String> {
        self.lock().unwrap().try_wallet_transaction(txid)
    }

    fn is_bitcoind(&self) -> bool {
        self.lock().unwrap().is_bitcoind()
    }

    fn backend_id(&self) -> Option<BackendId> {
        self.lock().unwrap().backend_id()
    }

    fn genesis_block_timestamp(&self) -> Result<u32, GenesisError> {
        self.lock().unwrap().genesis_block_timestamp()
    }

    fn genesis_block(&self) -> Result<BlockChainTip, GenesisError> {
        self.lock().unwrap().genesis_block()
    }

    fn sync_progress(&self) -> SyncProgress {
        self.lock().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.lock().unwrap().chain_tip()
    }

    fn try_chain_tip(&self) -> Result<BlockChainTip, String> {
        self.lock().unwrap().try_chain_tip()
    }

    fn chain_tip_result(&self) -> Result<BlockChainTip, String> {
        self.lock().unwrap().chain_tip_result()
    }

    fn try_sync_progress(&self) -> Result<SyncProgress, String> {
        self.lock().unwrap().try_sync_progress()
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
        descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
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

    fn rescan_progress_result(&self) -> Result<Option<f64>, String> {
        self.lock().unwrap().rescan_progress_result()
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

    fn mempool_spenders_result(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<Vec<MempoolEntry>, String> {
        self.lock().unwrap().mempool_spenders_result(outpoints)
    }

    fn mempool_entry_result(&self, txid: &bitcoin::Txid) -> Result<Option<MempoolEntry>, String> {
        self.lock().unwrap().mempool_entry_result(txid)
    }

    fn sees_mempool(&self) -> bool {
        self.lock().unwrap().sees_mempool()
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

#[cfg(test)]
mod imported_history_projection_tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn committed_generation_belongs_to_fetched_result_not_later_started_scan() {
        let cache = super::HistorySyncCache::default();
        cache.begin_full_scan(200);
        cache.branch_address_checked(false);
        cache.full_scan_fetched();
        cache.poll_failed("retryable poll commit", 100);
        cache.begin_full_scan(200);
        cache.branch_address_checked(true);
        cache.poll_failed("new scan failed before fetch", 100);
        cache.poll_succeeded(100);
        let first = cache.snapshot();
        assert_eq!(first.full_scan_generation, 2);
        assert_eq!(first.full_scan_completed_generation, 1);
        assert_eq!(first.full_scan_completed_at, Some(100));
        assert_eq!(first.addresses_checked, 0);
        assert!(!first.full_scan_in_progress);
        cache.begin_full_scan(200);
        cache.full_scan_fetched();
        cache.poll_succeeded(100); // Same second, different actual operation.
        let retry = cache.snapshot();
        assert_eq!(retry.full_scan_generation, 3);
        assert_eq!(retry.full_scan_completed_generation, 3);
        assert_eq!(retry.addresses_checked, 0);
    }

    #[test]
    fn historical_projection_includes_outputs_confirmed_before_stored_tip() {
        let outpoint = bitcoin::OutPoint {
            txid: bitcoin::Txid::all_zeros(),
            vout: 0,
        };
        let coin = Coin {
            outpoint,
            amount: bitcoin::Amount::from_sat(42_000),
            derivation_index: ChildNumber::from_normal_idx(250).unwrap(),
            is_change: false,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 100,
                time: 1,
            }),
            spend_txid: None,
            spend_block: None,
        };
        let coins = HashMap::from([(outpoint, coin)]);
        let persisted_tip = BlockChainTip {
            hash: bitcoin::BlockHash::all_zeros(),
            height: 150,
        };

        assert!(received_coins_from(&coins, Some(&persisted_tip)).is_empty());
        let discovered = received_coins_from(&coins, None);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].outpoint, outpoint);
        assert!(matches!(
            discovered[0].address,
            UTxOAddress::DerivIndex(index, false) if u32::from(index) == 250
        ));
    }
}

#[cfg(test)]
mod genesis_tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn unavailable_esplora_returns_genesis_error_and_no_rescan_block() {
        const DESCRIPTOR: &str = concat!(
            "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4z",
            "LqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJT",
            "Xc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8Uut",
            "BsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn"
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let client = esplora::client::Client::new(
            &crate::config::EsploraConfig {
                addr,
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            },
            sync::Arc::new(sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        let wallet = electrum::wallet::BdkWallet::new(
            &DESCRIPTOR.parse().unwrap(),
            bitcoin::BlockHash::all_zeros(),
            None,
            &[],
            &[],
            0.into(),
            0.into(),
        );
        let backend = esplora::Esplora::new(client, wallet, false).unwrap();
        let error = backend.genesis_block().unwrap_err();
        assert!(
            matches!(error, GenesisError::Esplora(error) if matches!(*error, esplora::client::Error::AllCooling))
        );
        assert_eq!(backend.block_before_date(0), None);
    }
}

#[cfg(test)]
mod history_sync_cache_tests {
    use super::*;

    #[test]
    fn eager_request_survives_a_full_queue_until_the_poller_consumes_it() {
        let cache = HistorySyncCache::default();
        cache.request_eager_sync();
        cache.request_eager_sync();
        assert!(cache.take_eager_sync_request());
        assert!(!cache.take_eager_sync_request());
    }

    #[test]
    fn progress_counts_addresses_and_stays_below_complete() {
        let cache = HistorySyncCache::default();
        assert_eq!(cache.snapshot().full_scan_progress(), None);
        cache.begin_full_scan(4);
        assert_eq!(cache.snapshot().full_scan_progress(), Some(0.0));
        cache.address_checked();
        assert_eq!(cache.snapshot().full_scan_progress(), Some(0.25));
        // The expected count is an estimate; a scan that looks further must not
        // claim to be done before it is.
        for _ in 0..10 {
            cache.address_checked();
        }
        assert_eq!(cache.snapshot().full_scan_progress(), Some(0.99));
        // An unknown total still reads as a scan under way.
        cache.begin_full_scan(0);
        assert_eq!(cache.snapshot().full_scan_progress(), Some(0.0));
    }

    /// A scan retried against the next server starts counting again.
    #[test]
    fn a_restarted_scan_counts_from_zero() {
        let cache = HistorySyncCache::default();
        cache.begin_full_scan(10);
        for _ in 0..5 {
            cache.address_checked();
        }
        cache.begin_full_scan(10);
        assert_eq!(cache.snapshot().addresses_checked, 0);
    }

    #[test]
    fn branch_coverage_is_available_only_after_a_fetched_scan_and_survives_failed_poll() {
        let cache = HistorySyncCache::default();
        cache.begin_full_scan(400);
        cache.branch_address_checked(false);
        cache.branch_address_checked(false);
        cache.branch_address_checked(true);
        assert_eq!(cache.pending_full_scan_coverage(), None);
        cache.full_scan_fetched();
        assert_eq!(cache.pending_full_scan_coverage(), Some((2, 1)));
        cache.poll_failed("db write", 5);
        assert_eq!(cache.pending_full_scan_coverage(), Some((2, 1)));
        cache.poll_succeeded(7);
        assert_eq!(cache.pending_full_scan_coverage(), None);
        cache.begin_full_scan(400);
        assert_eq!(cache.snapshot().addresses_checked, 0);
        cache.branch_address_checked(true);
        cache.full_scan_fetched();
        assert_eq!(cache.pending_full_scan_coverage(), Some((0, 1)));
    }

    #[test]
    fn completion_waits_for_the_poll_that_commits_the_scan() {
        let cache = HistorySyncCache::default();
        cache.begin_full_scan(10);
        cache.full_scan_fetched();
        let snapshot = cache.snapshot();
        assert!(!snapshot.full_scan_in_progress);
        assert_eq!(snapshot.full_scan_completed_at, None);
        cache.poll_failed("db write", 5);
        assert_eq!(cache.snapshot().full_scan_completed_at, None);
        cache.poll_succeeded(7);
        assert_eq!(cache.snapshot().full_scan_completed_at, Some(7));
        // A later ordinary poll does not move the completion time.
        cache.poll_succeeded(9);
        assert_eq!(cache.snapshot().full_scan_completed_at, Some(7));
    }

    #[test]
    fn failure_runs_count_up_and_flag_a_changed_error() {
        let cache = HistorySyncCache::default();
        let (failure, changed) = cache.poll_failed("timeout", 10);
        assert!(changed);
        assert_eq!(
            (failure.since, failure.last_at, failure.consecutive),
            (10, 10, 1)
        );
        let (failure, changed) = cache.poll_failed("timeout", 20);
        assert!(!changed);
        assert_eq!(
            (failure.since, failure.last_at, failure.consecutive),
            (10, 20, 2)
        );
        let (failure, changed) = cache.poll_failed("429 Too Many Requests", 30);
        assert!(changed);
        assert_eq!((failure.since, failure.consecutive), (10, 3));
        cache.poll_succeeded(40);
        assert_eq!(cache.snapshot().last_failure, None);
        let (failure, _) = cache.poll_failed("timeout", 50);
        assert_eq!((failure.since, failure.consecutive), (50, 1));
    }

    /// Server error text often carries request ids or timestamps; a repeat of the same
    /// failure must not count as a new error and log a warning on every poll.
    #[test]
    fn a_failure_differing_only_in_numbers_is_the_same_error() {
        let cache = HistorySyncCache::default();
        cache.poll_failed("upstream error (request id 1234, at 1791329974)", 10);
        let (_, changed) = cache.poll_failed("upstream error (request id 9876, at 1791330004)", 20);
        assert!(!changed);
        let (_, changed) = cache.poll_failed("connection refused", 30);
        assert!(changed);
        // A status code is not noise: a rate limit turning into an outage is news.
        cache.poll_failed("HttpResponse { status: 429, message: \"slow down\" }", 40);
        let (_, changed) =
            cache.poll_failed("HttpResponse { status: 503, message: \"slow down\" }", 50);
        assert!(changed);
    }

    #[test]
    fn expected_addresses_cover_revealed_indices_plus_a_gap_per_keychain() {
        // A restored Vault that has revealed nothing yet: a gap on each keychain.
        assert_eq!(expected_full_scan_addresses(2, [], 200), 400);
        // Receive index 2 and change index 0 revealed.
        assert_eq!(expected_full_scan_addresses(2, [2, 0], 200), 404);
    }
}
