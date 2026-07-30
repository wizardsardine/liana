//! Revalidating the managed node's chain state after a Core↔Knots flavour swap.
//!
//! Bitcoin Knots can enforce BIP-110 / RDTS (`consensusrules=rdts`); Bitcoin Core
//! cannot. Both flavours of the managed node share one datadir, and swapping
//! between them leaves stale rule-enforcement state behind in **both** directions:
//!
//! * **Core → Knots.** Knots adopts the inherited chainstate as-is — verified on
//!   the shipped binaries, it emits zero `UpdateTip` lines on startup — so history
//!   Core accepted is never checked against RDTS rules.
//! * **Knots → Core.** `BLOCK_FAILED_VALID` marks are persisted in the block index
//!   and survive a binary swap, so Core inherits Knots' rejections and stays on a
//!   minority chain while the most-work branch remains flagged `invalid`. Core
//!   never reconsiders those blocks on its own.
//!
//! The two are not equally cheap. Knots → Core is a single idempotent
//! `reconsiderblock` that disconnects nothing and needs no block data. Core → Knots
//! has to *rewind* the chain with `invalidateblock` so Knots can replay it, which
//! manufactures a deep reorg under every Vault's poller and can only reach as far
//! back as pruning has left block data. It therefore comes with a prune floor, a
//! maintenance pause, and a durable record of the rewind — see [`execute_replay`].
//!
//! # Unverified premise
//!
//! The rewind assumes that blocks reconnected by `reconsiderblock` are re-checked
//! against RDTS rather than fast-tracked on cached block-index validity. That has
//! **not** been demonstrated: RDTS does not exist on regtest, testnet4 activation
//! cannot be forced, and mainnet has not reached the window. Confirm it on a real
//! chain before RDTS locks in — until then the deployment gate keeps all of this
//! inert. See `plans/PLAN-rdts-flavor-swap-revalidation.md`, Task 0.

use std::{
    io,
    path::{Path, PathBuf},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use coincube_core::miniscript::bitcoin::Network;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    dir::CoincubeDirectory,
    node::bitcoind::{internal_bitcoind_directory, NodeFlavor, NodeIdentity},
};

/// Name of the BIP-110 / RDTS deployment in `getdeploymentinfo`.
///
/// Note it is `reduced_data`, *not* `rdts` — the latter is only the value passed
/// to `consensusrules`.
pub const RDTS_DEPLOYMENT: &str = "reduced_data";

/// Last mainnet block that cannot differ between a Core- and a Knots-built chain.
///
/// RDTS's mandatory-signalling window runs 961,632–963,647, so 961,632 is the
/// earliest height at which an enforcing node can reject a block a non-enforcing
/// node accepts, and its parent is the deepest point the two must agree on.
///
/// This is a constant because it cannot be derived at runtime: `getdeploymentinfo`
/// exposes the deployment's `status`, `since`, `min_activation_height` and
/// `max_activation_height`, but not the start of the mandatory-signalling window.
/// It is corroborated by the parameters the Knots binary does report —
/// `max_activation_height` is 965,664, and 961,632 / 963,648 / 965,664 are the
/// 477th, 478th and 479th retarget boundaries, i.e. the window is exactly one
/// retarget period. See [`RDTS_DEPLOYMENT`] for the runtime liveness gate that
/// keeps this height from being acted on before the fork is real.
pub const RDTS_ANCHOR_MAINNET: i32 = 961_631;

/// Blocks of headroom we refuse to rewind into above `pruneheight`.
///
/// Load-bearing, not cosmetic. `pruneheight` advances whenever bitcoind flushes,
/// so it can move between the moment we read it and the moment we call
/// `invalidateblock` — and disconnecting a block whose data has been pruned aborts
/// the node outright rather than returning an error. The margin buys us that race.
pub const PRUNE_SAFETY_MARGIN: i32 = 1_000;

/// The anchor for `network`, or `None` where we cannot place one.
///
/// Knots ships `reduced_data` on mainnet and testnet4 only; regtest and signet
/// carry no such deployment, so the feature is inherently inert there.
///
/// Testnet4 is excluded for a different reason: it *has* the deployment, but its
/// `getdeploymentinfo` output carries no `max_activation_height` and publishes no
/// mandatory-signalling window we can anchor to, so we have no defensible height.
/// Guessing one would risk clearing failure flags from the wrong depth. It should
/// be added once the testnet4 spike reads the real window out of the binary —
/// that is also what would make this code exercisable end to end before mainnet
/// activation, so it is worth doing.
pub fn rdts_anchor_height(network: Network) -> Option<i32> {
    match network {
        Network::Bitcoin => Some(RDTS_ANCHOR_MAINNET),
        // TODO(testnet4): add once the spike pins down its signalling window.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// Everything the planner needs to decide whether a swap requires remediation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainFacts {
    pub network: Network,
    /// The flavour the node ran as last time we observed it, if we ever did.
    pub previous_flavor: Option<NodeFlavor>,
    /// The flavour it is running as now.
    pub current_flavor: NodeFlavor,
    /// Blocks validated toward the best known tip.
    pub blocks: i32,
    /// Highest block on ANY branch the node knows of, including ones it rejected or
    /// has headers for but never downloaded. Compared against [`Self::blocks`] this
    /// is what tells us the node is following less work than it knows about.
    pub best_known_height: i32,
    /// Whether we have positive evidence the RDTS deployment will never activate.
    ///
    /// Note the polarity: this is set only on proof of failure, never on absence of
    /// proof. Bitcoin Core does not define the deployment at all, so a Knots → Core
    /// swap can only ever see "no such deployment" — and treating that as "not live"
    /// made the entire repair unreachable on the very node that needs it.
    pub rdts_abandoned: bool,
    /// How much history the node still stores. Nothing at or below the prune height
    /// can be disconnected — the data needed to do so is gone.
    pub prune_state: coincubed::PruneState,
}

/// Why no remediation is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// RDTS isn't deployed on this network (regtest, signet, …).
    NoDeploymentOnNetwork,
    /// The deployment timed out without activating, so its rules will never be
    /// enforced and nothing can diverge over them.
    DeploymentAbandoned,
    /// No branch the node knows of reaches past the first height at which the
    /// flavours can diverge. Until mainnet passes 961,632 this is the universal
    /// answer.
    NothingAboveAnchor,
    /// The node follows the best chain it knows of and did not just come from
    /// Knots. Nothing to clear.
    NothingToClear,
    /// Core → Knots, but pruning has already discarded everything above the
    /// anchor, so there is nothing left we are able to re-check.
    NothingRetainedAboveAnchor,
    /// Core → Knots, but the node reported a prune height we could not read, so we
    /// cannot establish that any disconnect is safe. Declining is the only option:
    /// disconnecting into pruned data aborts the node.
    PruneHeightUnknown,
}

/// What to do about the observed flavour change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevalidationPlan {
    Skip(SkipReason),
    /// Knots → Core. Clear the `BLOCK_FAILED_*` flags Knots left on the anchor's
    /// descendants so Core can reorg to the most-work chain it would have
    /// followed natively.
    ClearFailureFlags {
        anchor_height: i32,
    },
    /// Core → Knots. Disconnect back to `floor_height` and let Knots reconnect
    /// everything above it, so blocks Core accepted are finally checked against
    /// RDTS rules.
    ///
    /// `floor_height` is the deeper of the anchor and the prune floor: blocks below
    /// `pruneheight` cannot be disconnected at all, so re-checking everything the
    /// node still holds is the most that any mechanism could achieve here. When the
    /// floor sits above the anchor the coverage is partial, and the UI must say so
    /// rather than imply the whole chain was verified.
    ReplayUnderRdts {
        floor_height: i32,
        target_height: i32,
    },
}

impl RevalidationPlan {
    /// Whether this plan re-checks every block that could possibly have diverged,
    /// or only the tail that pruning left us.
    pub fn is_full_coverage(&self) -> bool {
        match self {
            Self::ReplayUnderRdts { floor_height, .. } => *floor_height <= RDTS_ANCHOR_MAINNET,
            _ => true,
        }
    }
}

/// Decide what the node's current state requires. Pure — all I/O happens in the
/// caller, which is what makes every branch below directly testable.
pub fn plan(facts: ChainFacts) -> RevalidationPlan {
    let Some(anchor_height) = rdts_anchor_height(facts.network) else {
        return RevalidationPlan::Skip(SkipReason::NoDeploymentOnNetwork);
    };
    if facts.rdts_abandoned {
        return RevalidationPlan::Skip(SkipReason::DeploymentAbandoned);
    }
    // Measured against the best branch the node knows of, not the one it follows: a
    // node parked at the anchor because it rejected the block above it has an active
    // tip of exactly `anchor_height`, and gating on that would refuse to repair the
    // very first rejection.
    if facts.best_known_height <= anchor_height {
        return RevalidationPlan::Skip(SkipReason::NothingAboveAnchor);
    }

    // The node is following less work than it knows about. Kept as an observation
    // rather than a stored flag so a swap we failed to record still surfaces.
    let node_stranded = facts.best_known_height > facts.blocks;

    match facts.current_flavor {
        NodeFlavor::Core => {
            // Two independent triggers. The ledger notices the swap immediately;
            // the stranded check catches one we failed to record — a swap made
            // through the installer, a lost or corrupt sidecar, a datadir moved
            // between machines.
            let came_from_knots = facts.previous_flavor == Some(NodeFlavor::Knots);
            if came_from_knots || node_stranded {
                RevalidationPlan::ClearFailureFlags { anchor_height }
            } else {
                RevalidationPlan::Skip(SkipReason::NothingToClear)
            }
        }
        NodeFlavor::Knots => {
            // A Knots node trailing the most-work chain is not a bug — that is
            // precisely what enforcing RDTS against a non-compliant majority looks
            // like. Never "repair" it: clearing the flags would only re-validate
            // and re-reject the same blocks, every startup, forever. So the only
            // trigger here is an actual swap from Core, never `node_stranded`.
            if facts.previous_flavor != Some(NodeFlavor::Core) {
                return RevalidationPlan::Skip(SkipReason::NothingToClear);
            }
            // We can only disconnect blocks we still have. `pruneheight` climbs
            // forever while the anchor stays put, so past roughly the prune window
            // this floor — not the anchor — is what bounds the replay.
            let floor_height = match facts.prune_state {
                coincubed::PruneState::NotPruned => anchor_height,
                coincubed::PruneState::Pruned(prune_height) => {
                    anchor_height.max(prune_height + PRUNE_SAFETY_MARGIN)
                }
                // Unreadable height: we cannot prove any disconnect is safe, and the
                // penalty for being wrong is an aborted node.
                coincubed::PruneState::PrunedUnknown => {
                    return RevalidationPlan::Skip(SkipReason::PruneHeightUnknown)
                }
            };
            if floor_height >= facts.blocks {
                return RevalidationPlan::Skip(SkipReason::NothingRetainedAboveAnchor);
            }
            RevalidationPlan::ReplayUnderRdts {
                floor_height,
                target_height: facts.blocks,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The flavour ledger
// ---------------------------------------------------------------------------

/// Persistent record of how the managed node last ran.
///
/// Deliberately records the flavour **observed** from the started node's
/// `getnetworkinfo.subversion`, not the flavour that was configured or requested.
/// The configured value is not reliable: `select_managed_bitcoind_exe` falls back
/// to the other flavour's binary when the preferred one isn't installed, and the
/// installer and loader can start the node without going through the settings
/// switch at all.
///
/// Correctness does not depend on this file. It is an optimisation that lets a
/// swap be noticed immediately; if it is missing, stale, or corrupt, the stateless
/// check in [`needs_reconsider`] still catches a node stranded off the most-work
/// chain on the next start.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedNodeState {
    /// Flavour the node was observed running as, last time it started.
    pub last_run_flavor: Option<NodeFlavor>,
    /// Set while an `invalidateblock` has been issued and not yet undone.
    ///
    /// Unlike [`Self::last_run_flavor`] this one *is* load-bearing. The failure
    /// flag `invalidateblock` writes is persistent and there is nothing in the node
    /// itself saying we put it there, so if we die between the two calls the node
    /// sits below the invalidated block forever. This is the only record that lets
    /// the next start finish the job.
    #[serde(default)]
    pub rewind: Option<RewindInFlight>,
    /// The block a repair rewound the chain to, so every Vault's poller can adopt
    /// the result.
    ///
    /// Also load-bearing, and for a reason that only shows up after the repair
    /// succeeds: a repair that moves the chain by more than the poller's
    /// `MAX_REORG_DEPTH` is refused as implausible, forever, leaving Vaults pinned
    /// to a chain the node no longer has. Publishing the floor is what lets them
    /// tell "we did this" from "the backend is lying". Durable because adoption can
    /// outlive the session that performed the repair.
    ///
    /// Recorded and *published* at different times, deliberately. See
    /// [`publish_sanctioned_rollback`].
    #[serde(default)]
    pub sanctioned_rollback: Option<SanctionedRollback>,
}

/// The floor of a rollback we performed deliberately, and the node we performed it
/// on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SanctionedRollback {
    pub hash: String,
    pub height: i32,
    /// RPC endpoint of the node this authorises, as `ip:port`.
    ///
    /// The floor block is public — every node following the chain has it — so it
    /// cannot scope the exception on its own. Without this, an external `bitcoind`
    /// a Vault happens to be pointed at would satisfy the floor check and lose its
    /// depth guard for everything above it.
    pub node_addr: String,
    /// Fingerprint of that node's credentials, which an address alone cannot
    /// supply: a socket is a location, and a different node — or the same one
    /// rebuilt on a new datadir — can later occupy it. The managed node
    /// authenticates by a cookie file inside its own datadir, so this moves when
    /// the datadir does.
    #[serde(default)]
    pub node_credentials: String,
    /// Whether the repair this floor belongs to has been observed to *finish*.
    ///
    /// The record is written before the chain is touched, so on its own it says only
    /// "a repair was started". Arming the poller exception on that is unsafe — the
    /// reorg may still be in flight, and a poll could adopt a chain that is about to
    /// change again — but discarding it is worse, because nothing else would ever
    /// authorise the finished chain. So it persists as pending and is only ever
    /// published once something has confirmed the node is done: see
    /// [`publish_sanctioned_rollback`], which refuses to arm a pending floor, and
    /// [`resume_pending_repair`], which establishes the confirmation a restart lost.
    #[serde(default)]
    pub confirmed: bool,
    /// Identifies the repair that wrote this floor.
    ///
    /// A watcher can outlive the claim it started under — the flag-clearing path
    /// deliberately releases the node after a bounded window and keeps watching — so by
    /// the time it has something to confirm, the record may belong to a *different*
    /// repair that has since claimed the node and replaced it. Confirming by position
    /// ("whatever is stored") would then mark that newer, possibly mid-rewind operation
    /// as finished and arm an exception for it. Every confirmation therefore names the
    /// operation it believes it is confirming, and is refused if the record has moved on.
    #[serde(default)]
    pub operation_id: String,
}

/// A token identifying one chain-repair operation. Long enough that two repairs never
/// collide; it only has to be unique, not unguessable.
fn new_operation_id() -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

impl SanctionedRollback {
    /// The floor of a rollback about to be performed on `bitcoind`.
    fn at(
        floor: &coincube_core::miniscript::bitcoin::BlockHash,
        height: i32,
        id: coincubed::BackendId,
    ) -> Self {
        Self {
            hash: floor.to_string(),
            height,
            node_addr: id.addr.to_string(),
            node_credentials: id.credentials,
            confirmed: false,
            operation_id: new_operation_id(),
        }
    }
}

/// A rewind we have started and not yet confirmed finished.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewindInFlight {
    /// The block we invalidated; its parent is where the chain was rolled back to.
    pub invalidated_hash: String,
    /// Height of the block below the invalidated one — the reconnect anchor.
    pub floor_height: i32,
    /// Where the tip was before we started, so we know when we are whole again.
    pub target_height: i32,
}

impl ManagedNodeState {
    /// Sidecar path: `<datadir>/bitcoind/managed_node_state.json`.
    ///
    /// Alongside `inbound_tor.json` in the managed-node directory, not in a vault
    /// datadir: the managed node is shared by every vault, and vault datadirs are
    /// removed wholesale on delete.
    pub fn path(coincube_datadir: &CoincubeDirectory) -> PathBuf {
        internal_bitcoind_directory(coincube_datadir).join("managed_node_state.json")
    }

    /// Load the ledger, distinguishing "there is no sidecar yet" from "there is one
    /// and we could not read it".
    ///
    /// Only a missing file yields the default. A read error or a corrupt file is an
    /// error, because anything that acts on [`Self::rewind`] must not mistake "we
    /// couldn't tell" for "nothing is pending" — that would leave a node parked
    /// below an invalidated block with nothing left to release it.
    pub fn try_load(coincube_datadir: &CoincubeDirectory) -> io::Result<Self> {
        let path = Self::path(coincube_datadir);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        serde_json::from_str(&contents).map_err(io::Error::other)
    }

    /// Fail-safe view of [`Self::try_load`] for the flavour ledger, where "unknown"
    /// is a safe answer: the next start just records the truth rather than acting on
    /// a guess. Callers that touch [`Self::rewind`] must use `try_load` instead.
    pub fn load(coincube_datadir: &CoincubeDirectory) -> Self {
        Self::try_load(coincube_datadir).unwrap_or_else(|e| {
            let path = Self::path(coincube_datadir);
            warn!("unreadable managed-node state at {path:?} ({e}); treating as unknown");
            Self::default()
        })
    }

    /// Persist the ledger via a temp file and a rename, so an interrupted write
    /// cannot leave a half-written sidecar that would later parse as garbage.
    /// (The `inbound_tor.json` precedent writes in place; this one is on the
    /// startup path of every vault, so it is worth the extra care.)
    pub fn save(&self, coincube_datadir: &CoincubeDirectory) -> io::Result<()> {
        let path = Self::path(coincube_datadir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomically(&path, json.as_bytes())
    }

    /// Record the flavour the node was just observed running as, preserving any
    /// in-flight rewind.
    ///
    /// Reads with `try_load` rather than `load`: defaulting on an unreadable sidecar
    /// would write back a state with no `rewind`, silently erasing the only record
    /// that can release a parked node. Skipping the update is the safe failure.
    pub fn record_run(coincube_datadir: &CoincubeDirectory, flavor: NodeFlavor) {
        let mut state = match Self::try_load(coincube_datadir) {
            Ok(state) => state,
            Err(e) => {
                warn!("not recording the managed-node flavour: state unreadable ({e})");
                return;
            }
        };
        state.last_run_flavor = Some(flavor);
        if let Err(e) = state.save(coincube_datadir) {
            warn!("could not record managed-node flavour: {e}");
        }
    }

    /// Record (or clear) an in-flight rewind, preserving the flavour ledger.
    ///
    /// Returns whether the state is now on disk as asked. A rewind whose intent we
    /// could not persist must not be started, because we would have no way to
    /// finish it — so a read failure is reported as failure too, not papered over
    /// with a default.
    pub fn set_rewind(
        coincube_datadir: &CoincubeDirectory,
        rewind: Option<RewindInFlight>,
    ) -> bool {
        let mut state = match Self::try_load(coincube_datadir) {
            Ok(state) => state,
            Err(e) => {
                warn!("could not read managed-node state to record the rewind: {e}");
                return false;
            }
        };
        state.rewind = rewind;
        match state.save(coincube_datadir) {
            Ok(()) => true,
            Err(e) => {
                warn!("could not record the in-flight rewind: {e}");
                false
            }
        }
    }

    /// Retire a completed rewind and advance the flavour ledger in a single write.
    ///
    /// Two writes would have a window between them, and a crash inside it is the
    /// worst of both: `rewind` gone, so nothing knows a rewind ever happened, and
    /// `last_run_flavor` still Core, so the next start plans the whole hours-long
    /// replay again. One `save` makes the completion as atomic as the rename
    /// underneath it.
    pub fn finish_rewind(coincube_datadir: &CoincubeDirectory, flavor: NodeFlavor) -> bool {
        let mut state = match Self::try_load(coincube_datadir) {
            Ok(state) => state,
            Err(e) => {
                warn!("could not read managed-node state to retire the rewind: {e}");
                return false;
            }
        };
        state.rewind = None;
        state.last_run_flavor = Some(flavor);
        match state.save(coincube_datadir) {
            Ok(()) => true,
            Err(e) => {
                warn!("could not retire the completed rewind: {e}");
                false
            }
        }
    }

    /// Persist (or clear) the floor of a repair's rollback. Does **not** publish it.
    ///
    /// Returns whether it is now on disk. Callers must not begin a repair unless it
    /// is: an in-memory-only authorisation dies with the process, and the Vault that
    /// has to adopt the rollback may not open until a later session — at which point
    /// the permanent refusal this exists to prevent is back, with the chain already
    /// rewound.
    pub fn write_sanctioned_rollback(
        coincube_datadir: &CoincubeDirectory,
        floor: Option<SanctionedRollback>,
    ) -> bool {
        matches!(
            Self::write_sanctioned_rollback_tracked(coincube_datadir, floor),
            FloorWrite::Written
        )
    }

    /// Mark the recorded floor as belonging to a repair that has finished, and hand
    /// back the confirmed record so it can be published.
    ///
    /// The only route from pending to armed. `None` if there is nothing recorded or
    /// the write failed, in which case the floor stays pending and a later start
    /// establishes the confirmation again — better than arming something we could not
    /// record as settled.
    pub fn confirm_sanctioned_rollback(
        coincube_datadir: &CoincubeDirectory,
        operation_id: &str,
    ) -> Option<SanctionedRollback> {
        let mut state = Self::try_load(coincube_datadir)
            .inspect_err(|e| warn!("could not read managed-node state to confirm a repair: {e}"))
            .ok()?;
        let floor = state.sanctioned_rollback.as_mut()?;
        // Fails closed on anything but an exact match. A record that has moved on
        // belongs to a repair we know nothing about — very possibly one still in flight
        // — and marking *that* finished is precisely the confusion to avoid.
        if floor.operation_id != operation_id {
            warn!(
                "not confirming a chain repair: the recorded floor now belongs to a \
                 different operation"
            );
            return None;
        }
        floor.confirmed = true;
        let floor = floor.clone();
        state
            .save(coincube_datadir)
            .inspect_err(|e| warn!("could not record the repair as finished: {e}"))
            .ok()?;
        Some(floor)
    }

    /// Give a record written before the node identity existed the identity of the
    /// managed node we are actually talking to.
    ///
    /// Such a record names an address and nothing else, which any `bitcoind` on that
    /// port would match, so it cannot be published as-is. Dropping it is not an
    /// option either: nothing would then schedule another repair — a Knots → Knots
    /// restart plans nothing — so a Vault still holding the pre-repair chain would
    /// refuse the node's chain forever, which is the exact failure the record exists
    /// to prevent. Migrating is the only answer that neither widens the exception
    /// nor discards it.
    ///
    /// Only when the address still matches. If it does not, the record describes a
    /// node this datadir no longer talks to and carries no authority here.
    pub fn migrate_sanctioned_rollback(
        coincube_datadir: &CoincubeDirectory,
        node: &coincubed::BackendId,
        legacy: &coincubed::BackendId,
    ) {
        let Ok(mut state) = Self::try_load(coincube_datadir) else {
            return;
        };
        let Some(floor) = state.sanctioned_rollback.as_mut() else {
            return;
        };
        if floor.node_credentials == node.credentials {
            return;
        }
        // Recorded under the identity this node had before its datadir carried a
        // marker, and provably *this* node's, because that older fingerprint is one we
        // can still compute and it matches. Re-stamp it: the day a marker is installed
        // — which is any managed-node start after an upgrade — every authorisation
        // written before it would otherwise stop matching, stranding a Vault on the
        // pre-repair chain with nothing left to schedule another repair.
        if !floor.node_credentials.is_empty() {
            if floor.node_credentials == legacy.credentials {
                info!("re-stamping a repair record from before this datadir had an identity");
                floor.node_credentials = node.credentials.clone();
                if let Err(e) = state.save(coincube_datadir) {
                    warn!("could not migrate the repair record: {e}");
                }
            }
            return;
        }
        if floor.node_addr != node.addr.to_string() {
            warn!(
                "a repair record predating node identities names {:?}, but the managed node is \
                 at {}; discarding it rather than applying it to a different node",
                floor.node_addr, node.addr
            );
            state.sanctioned_rollback = None;
        } else {
            info!("adopting the managed node's identity for a repair record that predates it");
            floor.node_credentials = node.credentials.clone();
        }
        if let Err(e) = state.save(coincube_datadir) {
            warn!("could not migrate the repair record: {e}");
        }
    }

    /// Move an unreadable sidecar aside and start from a clean one.
    ///
    /// Only for a repair the user deliberately asked for. Every automatic path
    /// treats an unreadable sidecar as "a rewind may be pending" and declines,
    /// which is right — but it would also make the manual "Re-check chain" action
    /// impossible, leaving the user with the one instruction we give them and no
    /// way to carry it out. Renamed rather than deleted, so the original is still
    /// there to look at.
    /// Replace an unreadable sidecar with a state carrying nothing but `floor`, keeping
    /// the original as evidence.
    ///
    /// Crash-safe by construction, which a rename-away followed by a separate write is
    /// not: that leaves a window in which the canonical path does not exist at all, and a
    /// crash inside it reads to the next start as "nothing pending" — from a node that
    /// may be parked below an invalidated block, which is the very thing the unreadable
    /// sidecar was making it decline to plan from.
    ///
    /// So the evidence is taken as a *copy* while the canonical file stays where it is,
    /// and the canonical file then changes in one atomic replacement. A crash before that
    /// replacement leaves the original unreadable file in place and reconciliation fails
    /// closed; a crash after it leaves a valid pending record that the next start
    /// resumes. There is no durable moment in between.
    ///
    /// Returns the evidence path and the original bytes, so a repair that never reaches
    /// the chain can put things back the same atomic way.
    fn replace_unreadable(
        coincube_datadir: &CoincubeDirectory,
        floor: SanctionedRollback,
    ) -> PendingInstall {
        let canonical = Self::path(coincube_datadir);
        let original = match std::fs::read(&canonical) {
            Ok(original) => original,
            Err(e) => return PendingInstall::NotInstalled(e),
        };
        // Durable before anything else changes: if this fails we have touched nothing.
        let evidence = match write_evidence_copy(&canonical, &original) {
            Ok(evidence) => evidence,
            Err(e) => return PendingInstall::NotInstalled(e),
        };
        warn!("managed-node state at {canonical:?} is unreadable; kept a copy at {evidence:?}");
        let operation_id = floor.operation_id.clone();
        let replacement = Self {
            sanctioned_rollback: Some(floor),
            ..Self::default()
        };
        match replacement.save_tracked(coincube_datadir) {
            AtomicWrite::Durable => PendingInstall::Installed { evidence, original },
            // The canonical file already holds the pending record; only its durability is
            // in doubt. Deleting the evidence here — on the assumption that an error means
            // nothing happened — is exactly how a pending repair ends up with no surviving
            // copy of what it displaced.
            AtomicWrite::ReplacedNotDurable(e) => PendingInstall::InstalledNotDurable {
                evidence,
                original,
                error: e,
            },
            AtomicWrite::NotReplaced(e) => {
                // Classified as "not replaced", but check rather than assume: the cost of
                // being wrong is deleting the evidence for a repair that is in fact
                // installed. Whatever the canonical file says now is the answer.
                if Self::installed_operation(coincube_datadir).as_deref() == Some(&operation_id) {
                    return PendingInstall::InstalledNotDurable {
                        evidence,
                        original,
                        error: e,
                    };
                }
                let _ = remove_evidence(&evidence, canonical.parent());
                PendingInstall::NotInstalled(e)
            }
        }
    }

    /// [`Self::write_sanctioned_rollback`], reporting which side of the rename a failure
    /// fell on.
    ///
    /// The boolean form cannot express the case that matters: the rename lands before the
    /// directory entry is flushed, so a failure at the last step returns "no" over a
    /// canonical file that already holds the new floor. A caller that believes it means
    /// "nothing was written" will not restore what it displaced — and the sidecar is then
    /// left naming a pending repair that never started, in place of a confirmed one that
    /// Vaults were relying on.
    fn write_sanctioned_rollback_tracked(
        coincube_datadir: &CoincubeDirectory,
        floor: Option<SanctionedRollback>,
    ) -> FloorWrite {
        let expected = floor.as_ref().map(|floor| floor.operation_id.clone());
        let mut state = match Self::try_load(coincube_datadir) {
            Ok(state) => state,
            Err(e) => {
                warn!("could not read managed-node state to record the rollback floor: {e}");
                return FloorWrite::NotWritten(e);
            }
        };
        // Everything else in the sidecar is left exactly as it was: the flavour ledger and
        // any in-flight rewind belong to other concerns entirely.
        state.sanctioned_rollback = floor;
        match state.save_tracked(coincube_datadir) {
            AtomicWrite::Durable => FloorWrite::Written,
            AtomicWrite::ReplacedNotDurable(e) => FloorWrite::WrittenNotDurable(e),
            AtomicWrite::NotReplaced(e) => {
                // Verify rather than trust the classification. Believing "nothing
                // happened" when something did is what leaves a displaced record
                // unrestored, so the canonical file gets the casting vote.
                if Self::installed_operation(coincube_datadir) == expected {
                    FloorWrite::WrittenNotDurable(e)
                } else {
                    FloorWrite::NotWritten(e)
                }
            }
        }
    }

    /// The operation the canonical file currently names, if it holds a readable record.
    fn installed_operation(coincube_datadir: &CoincubeDirectory) -> Option<String> {
        Self::try_load(coincube_datadir)
            .ok()?
            .sanctioned_rollback
            .map(|floor| floor.operation_id)
    }

    /// [`Self::save`], reporting which side of the rename a failure fell on.
    fn save_tracked(&self, coincube_datadir: &CoincubeDirectory) -> AtomicWrite {
        let path = Self::path(coincube_datadir);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return AtomicWrite::NotReplaced(e);
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => write_atomically_tracked(&path, json.as_bytes()),
            Err(e) => AtomicWrite::NotReplaced(io::Error::other(e)),
        }
    }
}

/// How far writing a rollback floor into a readable sidecar got. Same three-way
/// distinction as [`AtomicWrite`], and for the same reason.
#[derive(Debug)]
enum FloorWrite {
    /// The canonical file still holds what it held.
    NotWritten(io::Error),
    /// The canonical file now holds the new floor, but durability was not confirmed.
    WrittenNotDurable(io::Error),
    /// Written and confirmed durable.
    Written,
}

/// How far installing a pending-repair record over an unreadable sidecar got.
#[derive(Debug)]
enum PendingInstall {
    /// The canonical file still holds the original bytes, and any evidence copy taken
    /// along the way has been removed. Nothing happened.
    NotInstalled(io::Error),
    /// The canonical file now holds the pending record, but we could not confirm it is
    /// durable. The evidence is kept, and the caller must not treat this as a repair it
    /// may act on — only as one it must be able to undo.
    InstalledNotDurable {
        evidence: PathBuf,
        original: Vec<u8>,
        error: io::Error,
    },
    /// Installed and confirmed durable.
    Installed {
        evidence: PathBuf,
        original: Vec<u8>,
    },
}

/// How many `.corrupt[.N]` evidence names to try before giving up. Small in tests so the
/// exhaustion path is reachable without creating a thousand files.
fn evidence_name_limit() -> u32 {
    #[cfg(not(test))]
    {
        1_000
    }
    #[cfg(test)]
    4
}

/// Write `contents` to the first free `.corrupt[.N]` beside `canonical`, and make it
/// durable before returning.
///
/// The name is reserved with `create_new`, which is atomic: two repairs racing — in this
/// process or another, since the maintenance claim is only process-wide — cannot be
/// handed the same name, and neither can overwrite evidence the other just wrote. That
/// also sidesteps Windows refusing a rename onto an existing destination, because nothing
/// is renamed onto anything here.
fn write_evidence_copy(canonical: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    use std::io::Write;

    for n in 0..evidence_name_limit() {
        let candidate = match n {
            0 => canonical.with_extension("corrupt"),
            n => canonical.with_extension(format!("corrupt.{n}")),
        };
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let written = (|| -> io::Result<()> {
            fail_if(Failpoint::EvidenceWrite)?;
            file.write_all(contents)?;
            fail_if(Failpoint::EvidenceSync)?;
            file.sync_all()?;
            fail_if(Failpoint::EvidenceDirSync)?;
            if let Some(parent) = canonical.parent() {
                sync_parent_dir(parent)?;
            }
            Ok(())
        })();
        match written {
            Ok(()) => return Ok(candidate),
            Err(e) => {
                // The name is reserved but the file is empty or half-written. Leaving it
                // would present a partial copy as completed evidence and burn one of a
                // finite set of names — enough repeated failures and manual repair is
                // impossible. Close it first, since Windows will not unlink an open file.
                drop(file);
                if let Err(cleanup) = remove_evidence(&candidate, canonical.parent()) {
                    // Failing closed and saying so: an incomplete file is still there, and
                    // pretending otherwise is how it gets mistaken for real evidence.
                    return Err(io::Error::other(format!(
                        "could not write the managed-node evidence copy ({e}), and the \
                         incomplete file left at {candidate:?} could not be removed \
                         ({cleanup}); delete it by hand before retrying the repair"
                    )));
                }
                return Err(e);
            }
        }
    }
    Err(io::Error::other(
        "too many quarantined managed-node state files already",
    ))
}

/// Remove an incomplete evidence file and make the removal itself durable, so the name
/// is genuinely free again rather than free-until-the-next-crash.
fn remove_evidence(candidate: &Path, parent: Option<&Path>) -> io::Result<()> {
    fail_if(Failpoint::EvidenceCleanup)?;
    match std::fs::remove_file(candidate) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    if let Some(parent) = parent {
        sync_parent_dir(parent)?;
    }
    Ok(())
}

/// Flush a directory entry, so a file created or renamed in it survives a crash.
///
/// Unix only, for the same reason [`write_atomically`] gives.
fn sync_parent_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Mark the repair identified by `operation_id` as finished and arm the poller
/// exception for it.
///
/// The only way an exception ever goes live, so that "the chain is final" and "pollers
/// may act on it" cannot drift apart: the durable record and the live slot are set from
/// one place, in that order.
///
/// The caller must own the chain-operation claim. Anyone who has released it wants
/// [`reclaim_confirm_and_publish`] instead.
fn confirm_and_publish(coincube_datadir: &CoincubeDirectory, operation_id: &str) {
    match ManagedNodeState::confirm_sanctioned_rollback(coincube_datadir, operation_id) {
        Some(floor) => publish_sanctioned_rollback(Some(&floor)),
        None => warn!(
            "did not arm this repair's rollback: it could not be recorded as finished, or the \
             record now belongs to another operation. A later start will sort it out."
        ),
    }
}

/// Same, for a watcher that has already given the node up.
///
/// Retakes the claim first. Without it, a watcher outliving its own operation can arm an
/// exception in the middle of somebody else's: the flag-clearing path releases the node
/// after a bounded window and keeps watching for hours, and in that time another repair
/// can claim the node, withdraw the live authorisation and start rewinding the chain.
/// Publishing then puts an exception back that the newer operation deliberately took
/// away, over a chain that is mid-rewind — the exact transient the claim exists to keep
/// pollers away from.
///
/// A bare guard, not a [`ChainOperation`]: this is exclusion only, with nothing to
/// withdraw or restore. Contention means someone else owns the node, so we leave the
/// floor pending and let a later start confirm it.
fn reclaim_confirm_and_publish(coincube_datadir: &CoincubeDirectory, operation_id: &str) {
    let Some(_guard) = coincubed::MaintenanceGuard::try_acquire() else {
        warn!(
            "another chain operation now owns the managed node, so this repair's rollback is \
             left pending rather than authorised over an operation in progress"
        );
        return;
    };
    confirm_and_publish(coincube_datadir, operation_id);
}

/// Hand the recorded floor to coincubed, so every poller in this process can tell a
/// rollback we caused from a backend misreporting one.
///
/// Publishing is deliberately *not* part of recording it. Maintenance stops pollers
/// from starting a poll; it cannot stop one already in flight, and the depth guard
/// is what protects those. Between `invalidateblock` and the end of the reconnect
/// the node sits at the rewind floor on a chain that is about to change again — and
/// an exception published then lets an in-flight poll roll a Vault back to the
/// floor mid-repair, which is precisely what the guard was there to prevent. So the
/// floor is persisted before the destructive call and published only once the chain
/// has reached a confirmed terminal state, while maintenance is still held.
fn publish_sanctioned_rollback(floor: Option<&SanctionedRollback>) {
    let sanction = floor.and_then(|floor| {
        // The one choke point, so no caller — including startup, which sees whatever
        // the last session left behind — can arm an exception for a repair that was
        // never seen to finish.
        if !floor.confirmed {
            return None;
        }
        let hash = match coincube_core::miniscript::bitcoin::BlockHash::from_str(&floor.hash) {
            Ok(hash) => hash,
            Err(e) => {
                warn!("unreadable repair rollback floor {:?}: {e}", floor.hash);
                return None;
            }
        };
        let addr = match floor.node_addr.parse() {
            Ok(addr) => addr,
            Err(e) => {
                warn!("unreadable repair node address {:?}: {e}", floor.node_addr);
                return None;
            }
        };
        // A record written before credentials were part of the identity cannot be
        // scoped to a node, and an exception that matches on address alone is one
        // any bitcoind on that port would satisfy. Drop it and let the repair be
        // planned again rather than widen it.
        if floor.node_credentials.is_empty() {
            warn!("repair record predates node-identity scoping; not re-arming it");
            return None;
        }
        Some(coincubed::SanctionedRollback {
            floor: coincubed::BlockChainTip {
                hash,
                height: floor.height,
            },
            node: coincubed::BackendId {
                addr,
                credentials: floor.node_credentials.clone(),
            },
        })
    });
    coincubed::set_sanctioned_rollback(sanction);
}

/// Write `contents` to `path` through a sibling temp file, flushing before the
/// rename so the visible file is either the old one or the complete new one.
///
/// The directory is fsynced after the rename as well. Without that, the file's
/// contents are durable but the directory entry pointing at them may not be, so a
/// crash can lose the rename and with it a rewind record — precisely the crash this
/// record exists to survive.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    match write_atomically_tracked(path, contents) {
        AtomicWrite::Durable => Ok(()),
        AtomicWrite::ReplacedNotDurable(e) | AtomicWrite::NotReplaced(e) => Err(e),
    }
}

/// How far an atomic replacement got.
///
/// The distinction is not pedantry. The rename happens before the directory entry is
/// flushed, so a failure at the last step returns an error over a destination that
/// *already holds the new contents* — and a caller that reads "error" as "nothing
/// happened" will clean up on a false premise. See
/// [`ManagedNodeState::replace_unreadable`], where getting this wrong deletes the only
/// surviving copy of the state a pending repair displaced.
#[derive(Debug)]
enum AtomicWrite {
    /// The destination was not touched; it still holds whatever it held.
    NotReplaced(io::Error),
    /// The destination now holds the new contents, but the directory entry naming them
    /// was not confirmed durable, so a crash could still lose it.
    ReplacedNotDurable(io::Error),
    /// Replaced, and confirmed durable.
    Durable,
}

/// [`write_atomically`], reporting which side of the rename a failure fell on.
fn write_atomically_tracked(path: &Path, contents: &[u8]) -> AtomicWrite {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    let staged = (|| -> io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        fail_if(Failpoint::StateWrite)?;
        file.write_all(contents)?;
        fail_if(Failpoint::StateSync)?;
        file.sync_all()
    })();
    if let Err(e) = staged {
        // Nothing has been renamed, so the destination is untouched; drop the partial
        // staging file rather than leave it for the next writer to trip over.
        let _ = std::fs::remove_file(&tmp);
        return AtomicWrite::NotReplaced(e);
    }
    if let Err(e) = fail_if(Failpoint::StateRename).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return AtomicWrite::NotReplaced(e);
    }

    // Past this line the destination has already changed. Unix only: Windows has no
    // equivalent (a directory can't be opened as a file without backup semantics), and
    // NTFS metadata journalling makes the rename durable there anyway.
    let synced = (|| -> io::Result<()> {
        fail_if(Failpoint::StateDirSync)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    match synced {
        Ok(()) => AtomicWrite::Durable,
        Err(e) => AtomicWrite::ReplacedNotDurable(e),
    }
}

/// Filesystem steps whose failure modes are worth exercising, since none of them can be
/// provoked from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failpoint {
    None,
    EvidenceWrite,
    EvidenceSync,
    EvidenceDirSync,
    EvidenceCleanup,
    StateWrite,
    StateSync,
    StateRename,
    StateDirSync,
}

#[cfg(test)]
thread_local! {
    /// A set, not a single point: the interesting cases are compound — a write that
    /// fails *and* a cleanup that then fails too.
    static ARMED_FAILPOINTS: std::cell::RefCell<Vec<Failpoint>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Fail here if a test has armed this point. Compiled out entirely otherwise.
#[inline]
fn fail_if(point: Failpoint) -> io::Result<()> {
    #[cfg(test)]
    if ARMED_FAILPOINTS.with(|armed| armed.borrow().contains(&point)) {
        return Err(io::Error::other(format!("injected failure at {point:?}")));
    }
    #[cfg(not(test))]
    let _ = point;
    Ok(())
}

// ---------------------------------------------------------------------------
// The stateless check
// ---------------------------------------------------------------------------

/// The highest block on any branch the node knows of, `getchaintips`-wide.
///
/// Includes branches it rejected and ones it only has headers for. Comparing this
/// against the active tip is the backstop that makes the flavour ledger
/// non-load-bearing: it asks "is the node following the most work it knows about?",
/// which a healthy node satisfies regardless of history.
///
/// Deliberately **not** keyed on a branch's status being `"invalid"`. A node that
/// rejects a block stops downloading that branch, so the honest most-work chain
/// commonly shows up as `headers-only` rather than `invalid` — gating on
/// `"invalid"` would miss exactly the stranded case this exists to catch.
pub fn best_known_height(tips: &[coincubed::ChainTipEntry]) -> Option<i32> {
    tips.iter().map(|t| t.height).max()
}

/// Observe how the managed node came up, record it, and remediate if the flavour
/// it is running under has left it following the wrong chain.
///
/// Called from every managed-node start path, because `Bitcoind::maybe_start` is
/// the one funnel the loader, the installer, and the settings switch all pass
/// through. `observed_flavor` must come from the node's own
/// `getnetworkinfo.subversion` — not from configuration, which can disagree with
/// what actually launched.
///
/// Best-effort throughout: a node that just started is more important than this
/// check, so every failure is logged and swallowed rather than blocking startup.
///
/// Returns promptly. The inline work is a handful of RPCs plus, at most, one
/// fire-and-forget `reconsiderblock`. A Core → Knots replay is *not* inline —
/// it can run for hours, and this sits on the startup path of every Vault — so it
/// is handed to a background thread. See [`execute_replay`].
pub fn reconcile_after_start(
    coincube_datadir: &CoincubeDirectory,
    bitcoind: &coincubed::BitcoinD,
    config: &coincubed::config::BitcoindConfig,
    identity: &NodeIdentity,
    network: Network,
    observed_flavor: NodeFlavor,
) {
    // Nothing here may run against a provisional identity. Every branch below either
    // records a repair, arms an authorisation, or advances the flavour ledger on the
    // strength of one — and all three would be written against an identity that changes
    // the moment the marker this start could not install finally lands. The node is
    // already up and syncing by now; declining to reconcile costs a retry on the next
    // start, which is what the durable records are for.
    if !identity.permits_chain_repair() {
        warn!(
            "skipping the managed-node chain check: the node's identity is not established \
             yet, so any repair recorded now would not be recognised later. It will be \
             retried on the next start."
        );
        return;
    }
    // Re-arm the poller exception first: a repair from an earlier session may still
    // be waiting for some Vault to adopt its rollback, and that Vault's poller can
    // start the moment we return.
    //
    // But only if no rewind is outstanding. A recorded floor plus an unfinished
    // rewind means the chain is parked mid-repair, not repaired — publishing then
    // would authorise pollers to adopt a temporary rollback that is about to be
    // undone. The recovery below republishes it once the chain settles.
    // A record written before repairs were scoped to a node names only an address.
    // Give it this node's identity while we have it, rather than let it be discarded
    // at publish time — nothing would schedule another repair, and a Vault still on
    // the pre-repair chain would refuse the node's chain for good.
    ManagedNodeState::migrate_sanctioned_rollback(
        coincube_datadir,
        &bitcoind.backend_id(),
        &bitcoind.legacy_backend_id(),
    );
    let recorded = ManagedNodeState::load(coincube_datadir);
    if recorded.rewind.is_none() {
        match recorded.sanctioned_rollback.as_ref() {
            // Seen to finish in an earlier session, so it is safe to arm now.
            Some(floor) if floor.confirmed => publish_sanctioned_rollback(Some(floor)),
            // Started and never seen to finish. Publishing it here is what would let a
            // poller adopt a chain still mid-reorg; leaving it alone is what would
            // strand a Vault forever. Neither: re-establish the confirmation the lost
            // session was going to provide, and arm it on the far side of that.
            Some(floor) => resume_pending_repair(coincube_datadir, config, identity, floor),
            None => {}
        }
    }

    // Before anything else: if a previous run died mid-rewind, the node is parked
    // below an invalidated block and will not move until we release it. Planning
    // must not run alongside that. The node's height is meaningless mid-rewind, and
    // a planner fed it draws exactly the wrong conclusion — a node parked at the
    // floor looks like "nothing left above the anchor", which records the current
    // flavour and retires the very swap the recovery is still working on.
    if !resume_pending_rewind(
        coincube_datadir,
        bitcoind,
        config,
        identity,
        observed_flavor,
    )
    .may_plan()
    {
        return;
    }

    let previous_flavor = ManagedNodeState::load(coincube_datadir).last_run_flavor;

    // Networks without an anchor cost us nothing: no RPCs at all. Nothing can be
    // planned there, so the flavour record is safe to advance immediately.
    if rdts_anchor_height(network).is_none() {
        ManagedNodeState::record_run(coincube_datadir, observed_flavor);
        return;
    }

    let status = match bitcoind.chain_status() {
        Ok(status) => status,
        Err(e) => {
            warn!("could not read chain status for the RDTS flavour check: {e}");
            return;
        }
    };
    // Absence is not evidence of failure. Core does not define this deployment at
    // all, so on a Knots → Core swap the only honest reading of `None` is "this
    // binary can't tell us", and the height gate decides instead.
    let rdts_abandoned = bitcoind
        .deployment_status(RDTS_DEPLOYMENT)
        .map(|d| d.has_failed())
        .unwrap_or(false);
    let best_known = match bitcoind.chain_tips() {
        Ok(tips) => best_known_height(&tips).unwrap_or(status.blocks),
        Err(e) => {
            warn!("could not read chain tips for the RDTS flavour check: {e}");
            status.blocks
        }
    };

    let plan = plan(ChainFacts {
        network,
        previous_flavor,
        current_flavor: observed_flavor,
        blocks: status.blocks,
        best_known_height: best_known.max(status.blocks),
        rdts_abandoned,
        prune_state: status.prune_state,
    });
    match plan {
        RevalidationPlan::Skip(reason) => {
            tracing::debug!("No RDTS revalidation needed ({reason:?}).");
            ManagedNodeState::record_run(coincube_datadir, observed_flavor);
        }
        RevalidationPlan::ClearFailureFlags { .. } => {
            // Idempotent and cheap, so advancing the record is safe even if it fails:
            // a node still following less work than it knows about is caught again by
            // the height comparison on the next start, with no reliance on the ledger.
            if let Err(e) = clear_failure_flags(coincube_datadir, config, identity, plan) {
                warn!("RDTS revalidation failed: {e}");
            }
            ManagedNodeState::record_run(coincube_datadir, observed_flavor);
        }
        RevalidationPlan::ReplayUnderRdts {
            floor_height,
            target_height,
        } => {
            // Deliberately do NOT record the flavour here. The Core → Knots branch is
            // the one case with no stateless backstop — a Knots node legitimately
            // trailing the majority chain must never be "repaired" — so the ledger is
            // the only thing that can trigger a retry. Advancing it before the replay
            // succeeded would turn any transient failure into a permanent skip, since
            // the next start would see Knots → Knots. The replay thread records it on
            // success instead.
            spawn_replay(
                coincube_datadir.clone(),
                config.clone(),
                identity,
                observed_flavor,
                floor_height,
                target_height,
            );
        }
    }
}

/// Run a Core → Knots replay on its own thread.
///
/// The replay disconnects and reconnects blocks and can take hours. Every caller of
/// [`reconcile_after_start`] is on a node-startup path — the loader at app launch,
/// the installer, the settings flavour switch — so doing this inline would stall
/// app startup, or leave the settings screen on "starting…" until it finished.
///
/// The thread opens its own RPC connection rather than borrowing the caller's, so
/// nothing here is tied to the lifetime of the start that triggered it.
fn spawn_replay(
    coincube_datadir: CoincubeDirectory,
    config: coincubed::config::BitcoindConfig,
    identity: &NodeIdentity,
    observed_flavor: NodeFlavor,
    floor_height: i32,
    target_height: i32,
) {
    // Claim maintenance here, not inside the thread, and hand the guard over. Several
    // Vaults can attach to the one shared node in the same instant; a check followed
    // by a set would let two of them each start rewinding it. The guard's Drop
    // releases on every exit path, including a spawn that never happens.
    let operation = match ChainOperation::claim(&coincube_datadir, identity) {
        Ok(operation) => operation,
        Err(ClaimRefused::Busy) => {
            info!("A chain replay is already running; not starting another.");
            return;
        }
        Err(ClaimRefused::UnstableIdentity) => {
            warn!(
                "not replaying the chain: the managed node's identity is not established, so \
                 the rollback could not be authorised for the node it was performed on"
            );
            return;
        }
    };
    let spawned = thread::Builder::new()
        .name("rdts-replay".to_string())
        .spawn(move || {
            // Held for the whole replay; released when this closure returns. If this
            // closure never runs, dropping it un-does the claim — including putting
            // back the authorisation it displaced, since nothing was touched.
            let mut operation = operation;
            let bitcoind = match coincubed::BitcoinD::new(&config, "rdts_replay".to_string()) {
                Ok(bitcoind) => bitcoind,
                Err(e) => {
                    warn!("could not connect to the managed node to replay the chain: {e}");
                    return;
                }
            };
            match execute_replay(
                &mut operation,
                &coincube_datadir,
                &bitcoind,
                floor_height,
                target_height,
            ) {
                // Only now has the swap actually been dealt with, so only now may the
                // ledger advance. Leaving it behind on failure is what makes the next
                // start retry, instead of seeing Knots → Knots and skipping forever —
                // this direction has no stateless backstop, because a Knots node
                // legitimately trailing the majority chain must never be "repaired".
                // Retiring the rewind and advancing the flavour is one write, not two.
                // A crash between them would leave `rewind` gone and the flavour still
                // Core, and the next start would plan the whole replay over again —
                // hours of disconnect and reconnect, against a chain already re-checked.
                Ok(()) => {
                    ManagedNodeState::finish_rewind(&coincube_datadir, observed_flavor);
                }
                Err(e) => warn!("RDTS replay failed: {e}"),
            }
        });
    if let Err(e) = spawned {
        warn!("could not spawn the chain-replay thread: {e}");
    }
}

/// Whether a rewind is still being worked on, and so whether planning may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryState {
    /// Nothing was pending. The caller may plan from the node's current height.
    Idle,
    /// A rewind is being finished, here or by another holder. The node's height is
    /// mid-operation and must not be planned from.
    InFlight,
    /// We could not tell whether a rewind is pending. Treated exactly like one that
    /// is: the node may be parked below an invalidated block, and planning from that
    /// height is what turns an unfinished rewind into a permanent one.
    Indeterminate,
}

impl RecoveryState {
    /// Whether the caller may plan from the node's current height. Only a positive
    /// "nothing is pending" earns that; both of the other answers mean the height on
    /// offer may describe a half-rewound chain.
    pub fn may_plan(self) -> bool {
        self == Self::Idle
    }
}

/// Finish a rewind that a previous run started but did not complete.
///
/// Without this, dying between `invalidateblock` and `reconsiderblock` leaves the
/// node parked below the invalidated block indefinitely: the failure flag is
/// persistent and nothing in the node records that we were mid-operation.
/// Idempotent, so running it when there is nothing to finish costs one RPC.
///
/// The recovery is *not* complete when `reconsiderblock` returns. That call is
/// fire-and-forget and the reconnect it starts can run for hours, so the record is
/// only retired once the tip is observed back at the height the rewind started
/// from — and retired together with the flavour, in one write. Clearing it on the
/// RPC returning would throw away the only thing that can retry: a node that then
/// crashes, or a reconnect that never lands, leaves neither a rewind to resume nor
/// a Core → Knots transition to notice.
///
/// Returns promptly. The waiting happens on its own thread, for the same reason
/// [`execute_replay`] does.
pub fn resume_pending_rewind(
    coincube_datadir: &CoincubeDirectory,
    bitcoind: &coincubed::BitcoinD,
    config: &coincubed::config::BitcoindConfig,
    identity: &NodeIdentity,
    observed_flavor: NodeFlavor,
) -> RecoveryState {
    // `try_load`, not `load`: an unreadable sidecar must not read as "nothing
    // pending". If a rewind really is in flight, defaulting here would leave the
    // node parked below the invalidated block with nothing left to release it, and
    // no sign of why.
    let state = match ManagedNodeState::try_load(coincube_datadir) {
        Ok(state) => state,
        Err(e) => {
            tracing::error!(
                "Could not read the managed-node state ({e}). If a chain rewind was left \
                 unfinished, this node may be stuck below the block it invalidated — check \
                 its height against the network, and use \"Re-check chain\" in Node settings \
                 to release it."
            );
            // Not `Idle`. The comment above is only honoured if the caller also
            // declines to plan: a node parked mid-rewind reports a height that reads
            // as "nothing left above the anchor", and acting on it records the
            // current flavour and retires the very swap that still needs replaying.
            return RecoveryState::Indeterminate;
        }
    };
    let Some(rewind) = state.rewind else {
        return RecoveryState::Idle;
    };

    // A replay running right now has a rewind recorded and a half-disconnected chain.
    // Another Vault attaching at that moment must not mistake it for a crash: issuing
    // `reconsiderblock` would cut the live replay off mid-disconnect and clear the
    // record out from under it, leaving the chain half-rewound with nothing to
    // finish it. Claiming maintenance is both that check and this recovery's own
    // exclusion, in one atomic step.
    let mut operation = match ChainOperation::claim(coincube_datadir, identity) {
        Ok(operation) => operation,
        Err(ClaimRefused::Busy) => {
            info!("A chain rewind is already being worked on; leaving it to its owner.");
            return RecoveryState::InFlight;
        }
        Err(ClaimRefused::UnstableIdentity) => {
            warn!(
                "a chain rewind is outstanding but the managed node's identity is not \
                 established; leaving the node parked rather than reconnecting it on an \
                 authorisation no Vault would recognise"
            );
            return RecoveryState::Indeterminate;
        }
    };
    // Committed at once, unlike the paths that are about to move the chain
    // themselves. This one inherits a chain that is *already* parked below an
    // invalidated block, so there is no earlier authorisation worth restoring — one
    // would describe a chain state that no longer exists, and could well match the
    // parked one.
    operation.commit();

    warn!(
        "A chain rewind from a previous run was left unfinished (floor {}, target {}). \
         Re-issuing reconsiderblock to release the node.",
        rewind.floor_height, rewind.target_height
    );
    let Some(anchor) = bitcoind.get_block_hash(rewind.floor_height) else {
        warn!(
            "No block at height {} to reconsider from; leaving the record in place so a \
             later start can retry.",
            rewind.floor_height
        );
        // Still in flight: the node is parked below an invalidated block, so its
        // height describes a half-rewound chain and nothing may be planned from it.
        return RecoveryState::InFlight;
    };

    let coincube_datadir = coincube_datadir.clone();
    let config = config.clone();
    let spawned = thread::Builder::new()
        .name("rdts-rewind-recovery".to_string())
        .spawn(move || {
            let _operation = operation;
            let bitcoind = match coincubed::BitcoinD::new(&config, "rdts_recovery".to_string()) {
                Ok(bitcoind) => bitcoind,
                Err(e) => {
                    warn!("could not connect to the managed node to finish the rewind: {e}");
                    return;
                }
            };
            // The rollback the interrupted rewind created is ours, so record the
            // authorisation before the reconsider and refuse to go on without it, for
            // the same reason the replay does: an authorisation we cannot persist is
            // one the Vault that needs it will never see. Recorded only — publishing
            // it now would let an in-flight poll adopt the parked chain, which is
            // about to move again. The rewind record stays, so a later start retries.
            let floor = SanctionedRollback::at(&anchor, rewind.floor_height, bitcoind.backend_id());
            if !ManagedNodeState::write_sanctioned_rollback(&coincube_datadir, Some(floor.clone()))
            {
                warn!(
                    "could not record the rollback floor; leaving the rewind for a later \
                     start rather than reconnecting a chain no Vault would adopt"
                );
                return;
            }
            // The block the interrupted rewind invalidated, which is what identifies
            // the branch when we come to confirm the reconnect. Unreadable means we
            // fall back to the signals that need no branch identity, not that we
            // guess.
            let invalidated =
                coincube_core::miniscript::bitcoin::BlockHash::from_str(&rewind.invalidated_hash)
                    .inspect_err(|e| {
                        warn!("unreadable invalidated block in the rewind record: {e}")
                    })
                    .ok();
            match reconnect_and_confirm(
                &bitcoind,
                &anchor,
                invalidated.as_ref(),
                rewind.floor_height,
                rewind.target_height,
            ) {
                // A confirmed terminal state. The chain is final, so the pollers may
                // be let at it and the record and flavour may retire — all while
                // maintenance is still held.
                Ok(Some(height)) => {
                    if height < rewind.target_height {
                        warn!(
                            "The recovered node settled at height {height} rather than {}.",
                            rewind.target_height
                        );
                    }
                    confirm_and_publish(&coincube_datadir, &floor.operation_id);
                    ManagedNodeState::finish_rewind(&coincube_datadir, observed_flavor);
                }
                // Still reconnecting when we stopped watching. The record stays, so
                // the next start picks the recovery up rather than losing both it and
                // the swap that caused it.
                Ok(None) => {
                    warn!("the rewind recovery had not finished when we stopped watching")
                }
                Err(e) => warn!("the rewind recovery did not confirm: {e}"),
            }
        });
    if let Err(e) = spawned {
        warn!("could not spawn the rewind-recovery thread: {e}");
    }
    RecoveryState::InFlight
}

/// Exclusive use of the managed node for an operation that moves its chain.
///
/// Claiming it withdraws whatever rollback authorisation is in force, and that
/// withdrawal is the point. A completed repair leaves a live exception naming this
/// node and a floor — and the next repair rewinds to the *same* floor on the same
/// node, so that stale exception matches the transient rewind perfectly. Recording
/// the new floor without publishing it is no protection while the old one is armed:
/// an in-flight poll, which maintenance cannot stop, would adopt the temporary
/// rollback on the strength of the previous repair's authorisation.
///
/// Withdrawing it is only right once the chain is actually going to move, though.
/// Everything between claiming the node and the first mutating call can fail —
/// opening a connection, reading the anchor, persisting the floor, spawning the
/// worker — and on those paths the earlier repair is still exactly as valid as it
/// was a moment ago. So this snapshots what it displaced and puts it back on drop,
/// unless [`Self::commit`] has been called to say the chain has been touched. The
/// alternative is a Vault that had not yet adopted the earlier repair being stranded
/// for the rest of the session by an operation that never even started.
pub struct ChainOperation {
    coincube_datadir: CoincubeDirectory,
    /// Dropped last, so the restore below still happens under maintenance.
    _guard: coincubed::MaintenanceGuard,
    /// The authorisation that was live when we claimed the node.
    displaced_live: Option<coincubed::SanctionedRollback>,
    /// What the durable record held, and whether it was even readable. `None` means
    /// we could not read it, so we must not write anything back over it.
    displaced_record: Option<Option<SanctionedRollback>>,
    /// Whether we have overwritten that record and so owe it a restore.
    record_written: bool,
    /// Set when this operation replaced an unreadable sidecar: the evidence copy it
    /// kept, and the original bytes, so the replacement can be undone the same atomic
    /// way it was made.
    replaced_unreadable: Option<(PathBuf, Vec<u8>)>,
    committed: bool,
}

/// Whether a chain operation could be claimed right now, without keeping it.
///
/// Exists so the identity gate can be asserted directly. Claiming and dropping is
/// harmless — an uncommitted [`ChainOperation`] restores everything it displaced.
pub fn probe_chain_operation(
    coincube_datadir: &CoincubeDirectory,
    identity: &NodeIdentity,
) -> Result<(), ClaimRefused> {
    ChainOperation::claim(coincube_datadir, identity).map(|_| ())
}

/// Why a chain operation could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRefused {
    /// Another chain operation already owns the node.
    Busy,
    /// The node's durable identity is not settled, so anything recorded now would name
    /// an identity later clients will not agree with.
    UnstableIdentity,
}

impl ChainOperation {
    /// Claim the node for an operation that will move its chain.
    ///
    /// Takes the identity because this is the one gate every such operation passes
    /// through — the replay, the crash recovery, and the flag-clearing repair — so
    /// requiring it here is what makes "no repair without a settled identity" true by
    /// construction rather than by each caller remembering to check.
    fn claim(
        coincube_datadir: &CoincubeDirectory,
        identity: &NodeIdentity,
    ) -> Result<Self, ClaimRefused> {
        if !identity.permits_chain_repair() {
            return Err(ClaimRefused::UnstableIdentity);
        }
        let guard = coincubed::MaintenanceGuard::try_acquire().ok_or(ClaimRefused::Busy)?;
        let displaced_live = coincubed::sanctioned_rollback();
        let displaced_record = ManagedNodeState::try_load(coincube_datadir)
            .ok()
            .map(|state| state.sanctioned_rollback);
        coincubed::set_sanctioned_rollback(None);
        Ok(Self {
            coincube_datadir: coincube_datadir.clone(),
            _guard: guard,
            displaced_live,
            displaced_record,
            record_written: false,
            replaced_unreadable: None,
            committed: false,
        })
    }

    /// Persist the floor this operation will rewind to, remembering what it displaced so
    /// a failure before the chain is touched can put it back.
    ///
    /// Two shapes, because the sidecar may not be readable. An unreadable one is exactly
    /// the state the automatic paths decline to plan from — and rightly, since it may be
    /// hiding a rewind that left the node parked — so only a deliberate repair, which has
    /// already claimed the node and settled its identity, gets to move past it. When it
    /// does, the original is kept as evidence and the canonical file is replaced in one
    /// atomic step; see [`ManagedNodeState::replace_unreadable`] for why it is never
    /// renamed away first.
    fn record_floor(&mut self, floor: SanctionedRollback) -> bool {
        if ManagedNodeState::try_load(&self.coincube_datadir).is_ok() {
            return match ManagedNodeState::write_sanctioned_rollback_tracked(
                &self.coincube_datadir,
                Some(floor),
            ) {
                FloorWrite::Written => {
                    self.record_written = true;
                    true
                }
                // Remembered but not reported as success, exactly as on the unreadable
                // path: remembering is what makes the rollback restore the record this
                // displaced, and refusing is what keeps the node untouched.
                FloorWrite::WrittenNotDurable(e) => {
                    warn!(
                        "the rollback floor was written but could not be confirmed durable \
                         ({e}); not touching the node"
                    );
                    self.record_written = true;
                    false
                }
                FloorWrite::NotWritten(e) => {
                    warn!("could not record the repair's rollback floor: {e}");
                    false
                }
            };
        }
        match ManagedNodeState::replace_unreadable(&self.coincube_datadir, floor) {
            PendingInstall::Installed { evidence, original } => {
                self.replaced_unreadable = Some((evidence, original));
                self.record_written = true;
                true
            }
            // Remembered but *not* reported as success. Remembering keeps the rollback
            // correct — the canonical file really did change, and dropping this operation
            // has to put it back. Refusing keeps the node untouched: the caller stops
            // before `reconsiderblock`, because intent we could not confirm durable is
            // intent that may not survive to be resumed.
            PendingInstall::InstalledNotDurable {
                evidence,
                original,
                error,
            } => {
                warn!(
                    "the pending repair was written but could not be confirmed durable \
                     ({error}); leaving the evidence at {evidence:?} and not touching the node"
                );
                self.replaced_unreadable = Some((evidence, original));
                self.record_written = true;
                false
            }
            PendingInstall::NotInstalled(e) => {
                warn!("could not record the rollback floor over an unreadable sidecar: {e}");
                false
            }
        }
    }

    /// The chain is about to change (or already has), so there is no going back to
    /// the previous authorisation.
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ChainOperation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Nothing was changed, so the previous repair's authorisation still describes
        // the chain accurately. Putting it back is the difference between "we
        // declined to start" and "we stranded whoever had not adopted it yet".
        if let Some((evidence, original)) = self.replaced_unreadable.take() {
            // The sidecar we replaced was unreadable, so the record we wrote in its place
            // is not better information — and leaving it reads as "nothing pending" to
            // the next start, from a node that may be parked mid-rewind. Put the original
            // back through the same atomic replacement, so the canonical path is never
            // absent, and only then drop the evidence for a repair that never happened.
            let canonical = ManagedNodeState::path(&self.coincube_datadir);
            match write_atomically_tracked(&canonical, &original) {
                AtomicWrite::Durable => {
                    let _ = remove_evidence(&evidence, canonical.parent());
                    warn!(
                        "the repair did not start, so the unreadable managed-node state was \
                         put back at {canonical:?}"
                    );
                }
                // The original is back but not confirmed durable, so the copy stays: it is
                // the only other record of those bytes.
                AtomicWrite::ReplacedNotDurable(e) => warn!(
                    "put the unreadable managed-node state back, but could not confirm it \
                     durable ({e}); keeping the copy at {evidence:?}"
                ),
                AtomicWrite::NotReplaced(e) => warn!(
                    "could not restore the unreadable managed-node state ({e}); the copy at \
                     {evidence:?} still holds it"
                ),
            }
        } else if self.record_written {
            if let Some(displaced) = self.displaced_record.take() {
                // Restoring is a write like any other, so it has the same ambiguity to
                // answer for — reverting to the boolean form here would just move the
                // problem rather than fix it.
                match ManagedNodeState::write_sanctioned_rollback_tracked(
                    &self.coincube_datadir,
                    displaced,
                ) {
                    FloorWrite::Written => {}
                    FloorWrite::WrittenNotDurable(e) => warn!(
                        "put the previous repair record back, but could not confirm it \
                         durable ({e})"
                    ),
                    FloorWrite::NotWritten(e) => warn!(
                        "could not put the previous repair record back ({e}); the sidecar \
                         still names a repair that never started, which a later start will \
                         try to resume"
                    ),
                }
            }
        }
        coincubed::set_sanctioned_rollback(self.displaced_live.take());
    }
}

/// How long to let each phase of a replay run before we stop watching it.
///
/// Expiry is not a failure of the *node* — it keeps working through the reconnect
/// on its own, and the in-flight record means a later start can still finish the
/// job. It is reported as an error all the same, because the only thing we know at
/// that point is that we stopped looking, and the record must survive to be
/// confirmed later rather than be retired on an assumption.
const DISCONNECT_DEADLINE: Duration = Duration::from_secs(30 * 60);
/// Shorter than the replay's: clearing the flags leaves nothing half-done, so this
/// bounds how long Vaults are held for an operation that is usually seconds.
const RECONSIDER_DEADLINE: Duration = Duration::from_secs(30 * 60);
/// How long to keep watching for completion *after* the node has been released, so a
/// reorg that outlasts the parked window still gets its authorisation armed in this
/// session rather than waiting for the next start.
const CONFIRM_DEADLINE: Duration = Duration::from_secs(6 * 60 * 60);
const RECONNECT_DEADLINE: Duration = Duration::from_secs(6 * 60 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Rewind the chain to `floor_height` and let the (now Knots) node reconnect it, so
/// blocks accepted under Core are finally checked against RDTS.
///
/// Every Vault's poller is parked for the duration: mid-rewind the node's tip is
/// meaningless, and recording it would clear the confirmation state of every coin
/// above the floor. The parking is a courtesy — the poller's own depth guard is
/// what actually prevents that — but it keeps Vaults from flapping into a warning
/// state during an operation the user deliberately asked for.
pub fn execute_replay(
    operation: &mut ChainOperation,
    coincube_datadir: &CoincubeDirectory,
    bitcoind: &coincubed::BitcoinD,
    floor_height: i32,
    target_height: i32,
) -> Result<(), String> {
    // Re-read the prune height immediately before committing. It advances on every
    // flush, and disconnecting into pruned data aborts the node outright rather
    // than failing the RPC — a fire-and-forget call would not even show us the
    // corpse.
    let status = bitcoind
        .chain_status()
        .map_err(|e| format!("could not re-check the prune height before rewinding: {e}"))?;
    match status.prune_state {
        coincubed::PruneState::NotPruned => {}
        coincubed::PruneState::Pruned(prune_height) => {
            if floor_height <= prune_height + PRUNE_SAFETY_MARGIN {
                return Err(format!(
                    "refusing to rewind to {floor_height}: pruning has reached \
                     {prune_height} and disconnecting pruned blocks would abort the node"
                ));
            }
        }
        coincubed::PruneState::PrunedUnknown => {
            return Err(
                "refusing to rewind: the node is pruned but did not report a readable \
                 prune height, so we cannot tell which blocks are safe to disconnect"
                    .to_string(),
            );
        }
    }

    let first_divergent = floor_height + 1;
    let invalidate_hash = bitcoind
        .get_block_hash(first_divergent)
        .ok_or_else(|| format!("no block at height {first_divergent} to rewind from"))?;
    let floor_hash = bitcoind
        .get_block_hash(floor_height)
        .ok_or_else(|| format!("no block at the rewind floor {floor_height}"))?;

    // Persist the intent BEFORE the destructive call. If this write fails we must
    // not proceed: the failure flag would outlive us with nothing to undo it.
    let rewind = RewindInFlight {
        invalidated_hash: invalidate_hash.to_string(),
        floor_height,
        target_height,
    };
    if !ManagedNodeState::set_rewind(coincube_datadir, Some(rewind)) {
        return Err(
            "could not record the rewind before starting it; refusing to proceed".to_string(),
        );
    }

    info!(
        "Rewinding the managed node to height {floor_height} so Knots can re-check blocks \
         {first_divergent}..{target_height} under BIP-110."
    );

    // The rollback about to happen is one we chose, so every poller will need to be
    // allowed to adopt it. Persisted before the disconnect and, like the rewind
    // record above, a hard precondition for it: an authorisation that only ever
    // lived in this process dies with it, and a Vault opened afterwards would refuse
    // the repaired chain permanently — with the rewind already done and no way back.
    //
    // Recorded, not published. Between the disconnect below and the end of the
    // reconnect the node sits at the floor on a chain that is about to change again;
    // an exception live during that window lets a poll already in flight — which
    // maintenance cannot stop — roll a Vault back to the floor mid-repair. It is
    // published at the bottom of this function, once the chain is final.
    let sanction = SanctionedRollback::at(&floor_hash, floor_height, bitcoind.backend_id());
    if !operation.record_floor(sanction.clone()) {
        // Undo the intent we just recorded: nothing destructive has happened yet, so
        // leaving a rewind record behind would send the next start looking for a
        // rewind that never started.
        ManagedNodeState::set_rewind(coincube_datadir, None);
        return Err(
            "could not record the rollback floor before starting the rewind; refusing to \
             proceed"
                .to_string(),
        );
    }

    // Past this line the chain has been touched, so the authorisation this operation
    // displaced can no longer be put back — it would describe a chain that no longer
    // exists, and its floor is very likely this one.
    operation.commit();
    bitcoind
        .invalidate_block_noreply(&invalidate_hash)
        .map_err(|e| format!("invalidateblock at height {first_divergent} failed: {e}"))?;
    // Only an observed disconnect may be built on. If we merely stopped watching,
    // the chain may still be anywhere, and the record we leave behind is what lets a
    // later start pick the recovery up. A disconnect has no "finished early" state
    // worth honouring — it either reaches the floor or it has not happened — so
    // nothing is treated as settled here.
    match wait_for_tip(
        bitcoind,
        DISCONNECT_DEADLINE,
        |blocks| blocks <= floor_height,
        |_| false,
    )? {
        TipWait::Reached(_) => {}
        TipWait::Settled(height) => {
            return Err(format!(
                "the rewind stalled at height {height} instead of reaching {floor_height}; \
                 leaving the record so a later start can finish it"
            ))
        }
        TipWait::Deadline => {
            return Err(
                "the rewind did not reach the floor before the deadline; leaving the record \
                 so a later start can finish it"
                    .to_string(),
            )
        }
    }

    info!("Rewind complete; reconnecting under BIP-110 rules.");
    match reconnect_and_confirm(
        bitcoind,
        &floor_hash,
        Some(&invalidate_hash),
        floor_height,
        target_height,
    )? {
        // The chain is final. Only now may the pollers be allowed past their depth
        // limit, and this still happens under maintenance — the caller holds it until
        // this function returns.
        Some(height) => {
            if height < target_height {
                // A real terminal state, not a failure: Knots refused something Core
                // had accepted.
                warn!(
                    "After re-checking, the node settled at height {height} rather than \
                     {target_height}. Blocks accepted under Core were rejected under BIP-110."
                );
            }
            confirm_and_publish(coincube_datadir, &sanction.operation_id);
            Ok(())
        }
        // We stopped watching mid-reconnect. The node carries on by itself, and the
        // record we leave behind lets a later start confirm and retire it.
        None => Err(
            "the reconnect had not finished when we stopped watching; leaving the record \
             so a later start can confirm it"
                .to_string(),
        ),
    }
}

/// How the wait ended. The distinction is the whole point: "we stopped watching"
/// must never be mistaken for "the node got there", because the caller retires a
/// durable recovery record on the strength of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TipWait {
    /// The predicate was satisfied — an observed, confirmed transition.
    Reached(i32),
    /// The node itself reported it is finished and will not advance further, short
    /// of the predicate. This is the shape a legitimate RDTS rejection takes: Knots
    /// refuses a block Core accepted, so the reconnect ends below the height it
    /// started from.
    Settled(i32),
    /// Neither happened before the deadline. Nothing may be concluded.
    Deadline,
}

/// Reconnect from `anchor` and establish, authoritatively, where the node ends up.
///
/// `Ok(Some(height))` is a confirmed terminal state; `Ok(None)` means we stopped
/// watching before reaching one and nothing may be concluded.
///
/// Two signals, because neither covers the whole space on its own:
///
/// 1. **`reconsiderblock` returning.** It re-activates the best chain before it
///    replies, so a reply means the node is done. This is the only thing that can
///    settle the awkward case — the node clears the flags, immediately re-rejects
///    the same block, and leaves the block index looking precisely as our own
///    `invalidateblock` left it. No amount of watching from outside distinguishes
///    that from a request that never arrived. It is also exactly the case that
///    finishes fast, so the call returns well inside the socket timeout.
/// 2. **Watching the chain**, for reconnects that legitimately outlast the socket.
///    Those are the ones that reconnected a great many blocks, and a node that got
///    that far has demonstrably acted on the request — which is what makes the fork
///    point usable as evidence in [`is_replay_branch_rejected`].
fn reconnect_and_confirm(
    bitcoind: &coincubed::BitcoinD,
    anchor: &coincube_core::miniscript::bitcoin::BlockHash,
    invalidated: Option<&coincube_core::miniscript::bitcoin::BlockHash>,
    floor_height: i32,
    target_height: i32,
) -> Result<Option<i32>, String> {
    let completed = bitcoind
        .reconsider_block(anchor)
        .map_err(|e| format!("reconsiderblock at height {floor_height} failed: {e}"))?;
    if completed {
        // The node finished within the timeout, so whatever it reports is final.
        let status = bitcoind
            .chain_status()
            .map_err(|e| format!("could not read the height the node settled at: {e}"))?;
        return Ok(Some(status.blocks));
    }
    match wait_for_tip(
        bitcoind,
        RECONNECT_DEADLINE,
        |blocks| blocks >= target_height,
        |blocks| replay_branch_rejected(bitcoind, blocks, floor_height, target_height, invalidated),
    )? {
        TipWait::Reached(height) | TipWait::Settled(height) => Ok(Some(height)),
        TipWait::Deadline => Ok(None),
    }
}

/// Whether the node has finished with the branch we asked it to re-check: that
/// branch is now marked invalid, so nothing more is coming from it.
///
/// The authoritative end of a reconnect, and the reason a stationary height is not
/// allowed to stand in for one. Validating a single block can outlast any stability
/// window we would be willing to wait, so from the outside "still working" and
/// "finished" look identical — and concluding the latter retires the only record
/// that can retry, on a node that may still be mid-reconsideration.
///
/// Identified by where the invalid branch sits, not by where the active tip stopped.
/// Those are not the same thing: having refused a block, the node is free to go and
/// follow a *compliant* alternative for a while, which leaves the invalid branch
/// forking well below the new tip. Keying on "forks exactly at the tip" would call
/// that node non-terminal forever — the deadline would expire, the rewind record
/// would survive, and every restart would replay the whole recovery.
///
/// Two things have to hold, and they answer different questions.
///
/// **Did the node act on the reconsider?** `blocks > floor_height`. Our own
/// `invalidateblock` landed on the block just above the floor, so before the
/// reconsider takes effect `getchaintips` already reports the original branch as
/// invalid and forking at exactly the floor. Reading that as "re-checked and
/// refused" would retire the recovery record with the chain still parked. What rules
/// it out is the tip itself having climbed above the floor, which cannot happen until
/// the node has re-activated something. Note this is the *tip*, not the fork point:
/// the node may well reject the first block above the floor and then activate a
/// compliant alternative that also forks at the floor, and requiring the fork to be
/// above the floor would refuse that perfectly ordinary outcome. The case where the
/// tip never climbs at all — refused the first block, no alternative to move to — has
/// no signature here and is settled by [`reconnect_and_confirm`] instead.
///
/// **Is this the branch we replayed?** Geometry narrows it to a branch reaching at
/// least as high as the chain did before we started and forking no deeper than the
/// floor, and then [`descends_from`] settles it: the candidate must actually descend
/// from the block we invalidated. Geometry alone is a coincidence away from being
/// wrong, and an unrelated invalid branch that happened to fit would retire the
/// record early.
///
/// `getchaintips` reports `invalid` only for a branch the node actually validated,
/// which is exactly this case: the blocks were already on disk and were re-checked
/// under the new rules. (Its other failure mode — a branch abandoned before
/// download, reported `headers-only` — cannot arise here.)
fn replay_branch_rejected(
    bitcoind: &coincubed::BitcoinD,
    blocks: i32,
    floor_height: i32,
    target_height: i32,
    invalidated: Option<&coincube_core::miniscript::bitcoin::BlockHash>,
) -> bool {
    // Without the block we invalidated there is no way to identify the branch, so
    // there is nothing to confirm here. `reconnect_and_confirm` still has the RPC
    // reply and a fully restored tip to go on.
    let Some(invalidated) = invalidated else {
        return false;
    };
    let tips = match bitcoind.chain_tips() {
        Ok(tips) => tips,
        Err(e) => {
            warn!("could not read chain tips to tell whether the reconnect finished: {e}");
            return false;
        }
    };
    let candidates: Vec<_> =
        candidate_rejected_branches(&tips, blocks, floor_height, target_height)
            .map(|tip| tip.hash)
            .collect();
    candidates
        .iter()
        .any(|hash| descends_from(bitcoind, hash, invalidated, floor_height + 1))
}

/// The branches that *could* be the one we replayed, on `getchaintips` geometry
/// alone. Separated from the RPCs so every shape of output can be exercised directly;
/// a candidate is only ever a candidate until [`descends_from`] agrees.
fn candidate_rejected_branches(
    tips: &[coincubed::ChainTipEntry],
    blocks: i32,
    floor_height: i32,
    target_height: i32,
) -> impl Iterator<Item = &coincubed::ChainTipEntry> {
    // The node has not acted on the reconsider yet, so any invalid branch on offer is
    // the one our own `invalidateblock` created.
    let acted = blocks > floor_height;
    tips.iter().filter(move |tip| {
        acted
            && tip.status == "invalid"
            && tip.height >= target_height
            && tip.height.saturating_sub(tip.branch_len) >= floor_height
    })
}

/// Whether the branch tipped by `tip` descends from `ancestor`, which sits at
/// `ancestor_height`.
///
/// Walks the branch's headers back to that height and compares. Fails closed: a walk
/// we cannot finish leaves the terminal state unconfirmed rather than assuming it,
/// which costs a retry at worst.
///
/// Only reached once a candidate has passed the geometric filter and the tip has sat
/// still for minutes, so paying one `getblockheader` per block of the branch is
/// affordable — and it is the only way to turn "a branch that looks like ours" into
/// "our branch".
fn descends_from(
    bitcoind: &coincubed::BitcoinD,
    tip: &coincube_core::miniscript::bitcoin::BlockHash,
    ancestor: &coincube_core::miniscript::bitcoin::BlockHash,
    ancestor_height: i32,
) -> bool {
    /// Ceiling on the walk, so a misreported branch cannot spin us forever.
    const MAX_STEPS: usize = 50_000;

    let mut cursor = *tip;
    for _ in 0..MAX_STEPS {
        if cursor == *ancestor {
            return true;
        }
        let Some(header) = bitcoind.get_block_stats(cursor) else {
            return false;
        };
        // Walked past where the ancestor would be without meeting it.
        if header.height <= ancestor_height {
            return false;
        }
        let Some(previous) = header.previous_blockhash else {
            return false;
        };
        cursor = previous;
    }
    warn!("gave up walking a rejected branch back to the block we invalidated");
    false
}

/// Watch until the node has demonstrably finished re-activating its chain, or until
/// `deadline`. `true` only on a positive confirmation.
fn watch_reorg(bitcoind: &coincubed::BitcoinD, baseline_work: &str, deadline: Duration) -> bool {
    match wait_for_tip(
        bitcoind,
        deadline,
        |_| false,
        |_| reorg_completed(bitcoind, baseline_work),
    ) {
        Ok(TipWait::Settled(height)) => {
            info!("The node settled at height {height} after clearing the flags.");
            true
        }
        Ok(_) => false,
        Err(e) => {
            warn!("could not watch the reorg after clearing the flags: {e}");
            false
        }
    }
}

/// Re-establish the confirmation a session that died mid-repair was going to provide.
///
/// A pending floor on disk means a `reconsiderblock` was issued and never seen to
/// finish. `reconsiderblock` is idempotent and disconnects nothing, so the honest way
/// to find out where the node ended up is to ask again and watch — which is what
/// [`clear_failure_flags`] does, so this simply is that, with the anchor taken from the
/// record instead of from a fresh plan.
///
/// Only ever a flag-clearing repair, by construction: a replay that got as far as
/// writing a floor also wrote a rewind record, and its caller only reaches here when
/// there is none. A replay's floor is therefore either confirmed or still guarded by
/// that record. Re-issuing at a replay floor would be harmless anyway — it is the same
/// idempotent call the recovery path makes.
///
/// Best-effort and off the startup path: the floor stays pending if this fails, and the
/// next start tries again.
fn resume_pending_repair(
    coincube_datadir: &CoincubeDirectory,
    config: &coincubed::config::BitcoindConfig,
    identity: &NodeIdentity,
    floor: &SanctionedRollback,
) {
    info!(
        "A chain repair from a previous run was never confirmed finished (floor {}). \
         Re-checking before authorising Vaults to adopt its rollback.",
        floor.height
    );
    if let Err(e) = clear_failure_flags(
        coincube_datadir,
        config,
        identity,
        RevalidationPlan::ClearFailureFlags {
            anchor_height: floor.height,
        },
    ) {
        warn!("could not resume the unconfirmed chain repair: {e}");
    }
}

/// The work in the chain the node is currently on.
fn active_chainwork(bitcoind: &coincubed::BitcoinD) -> Option<String> {
    let status = bitcoind.chain_status().ok()?;
    bitcoind.block_chainwork(&status.best_block_hash?)
}

/// Whether clearing the flags has demonstrably finished: the node has moved to a
/// chain carrying more work than it had before we asked, and has stopped moving.
///
/// The progress half is not decoration. [`following_best_known`] on its own is
/// satisfied by the state this operation exists to change — a stranded node is on the
/// only branch it has not rejected, so nothing it knows of outweighs it, and the
/// predicate reads as "finished" before the `reconsiderblock` has had any effect at
/// all. Requiring the work to have *increased* past what we measured beforehand is
/// what distinguishes a completed reorg from an untouched one.
fn reorg_completed(bitcoind: &coincubed::BitcoinD, baseline_work: &str) -> bool {
    match active_chainwork(bitcoind) {
        // No more work than before we asked: still the state we wanted changed.
        Some(work) if work.as_str() <= baseline_work => false,
        Some(_) => following_best_known(bitcoind),
        None => false,
    }
}

/// Whether the node is following the branch carrying the most work of any it knows
/// of and has not rejected.
///
/// Necessary for a `reconsiderblock` to be finished, but not sufficient on its own —
/// see [`reorg_completed`], which is what callers should ask.
///
/// On work, not height — the distinction that is merely academic elsewhere in this
/// module is load-bearing here. A branch can be shorter and still carry more work,
/// and a height comparison would call the node finished while such a branch is
/// pending, releasing maintenance and publishing the authorisation mid-reorg.
/// `getchaintips` does not report work, so each candidate costs a `getblockheader`;
/// this is only asked once the tip has sat still for minutes, so that is cheap.
///
/// A candidate we cannot weigh keeps us waiting rather than concluding.
fn following_best_known(bitcoind: &coincubed::BitcoinD) -> bool {
    let tips = match bitcoind.chain_tips() {
        Ok(tips) => tips,
        Err(e) => {
            warn!("could not read chain tips to tell whether the reorg finished: {e}");
            return false;
        }
    };
    let Some(active) = tips.iter().find(|tip| tip.is_active()) else {
        warn!("the node reported no active chain tip");
        return false;
    };
    let Some(active_work) = bitcoind.block_chainwork(&active.hash) else {
        return false;
    };
    !tips
        .iter()
        .filter(|tip| !tip.is_active() && tip.status != "invalid")
        .any(|tip| {
            bitcoind
                .block_chainwork(&tip.hash)
                .is_none_or(|work| work > active_work)
        })
}

/// Poll the node's height until `done`, until `settled` confirms it has stopped for
/// good, until the deadline expires, or until the node stops answering.
///
/// `settled` is consulted only once the height has held still for a while — it
/// costs an RPC, and asking it of a tip that is still climbing is pointless. It is
/// what turns "the number has not moved" into an actual terminal state; without it
/// this function cannot tell a finished node from a slow one.
///
/// A node that stops answering after `invalidateblock` is the fatal-abort case —
/// disconnecting pruned data kills bitcoind rather than returning an error — so a
/// run of consecutive failures is treated as a hard error, never as success.
fn wait_for_tip(
    bitcoind: &coincubed::BitcoinD,
    deadline: Duration,
    done: impl Fn(i32) -> bool,
    settled: impl Fn(i32) -> bool,
) -> Result<TipWait, String> {
    const MAX_CONSECUTIVE_FAILURES: u32 = 15;
    /// Consecutive readings at one height before we bother asking whether the node
    /// has finished. Generous, because a reconnect legitimately pauses while it
    /// validates, and re-checked every this many readings after that.
    const STABLE_POLLS: u32 = 150;

    let started = Instant::now();
    let mut last_seen: Option<i32> = None;
    let mut stable = 0u32;
    let mut failures = 0u32;
    while started.elapsed() < deadline {
        match bitcoind.chain_status() {
            Ok(status) => {
                failures = 0;
                if done(status.blocks) {
                    return Ok(TipWait::Reached(status.blocks));
                }
                if last_seen == Some(status.blocks) {
                    stable += 1;
                    if stable.is_multiple_of(STABLE_POLLS) && settled(status.blocks) {
                        return Ok(TipWait::Settled(status.blocks));
                    }
                } else {
                    stable = 0;
                    last_seen = Some(status.blocks);
                }
            }
            Err(e) => {
                failures += 1;
                // A gap in observation is not evidence the tip held still: the node
                // may well have moved while we could not see it.
                stable = 0;
                last_seen = None;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(format!(
                        "the managed node stopped responding during the rewind ({e}). It may have \
                         shut down; check its debug.log before restarting."
                    ));
                }
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(TipWait::Deadline)
}

/// Clear the inherited block-index rejections from the RDTS anchor, so the node can
/// reorg onto the most-work chain it would have followed natively.
///
/// Claims the node for the duration, exactly like a replay does. Not a formality:
/// `reconsiderblock(anchor)` clears the `BLOCK_FAILED_*` marks on the anchor's
/// descendants, and a replay's `invalidateblock` mark is one of them — so firing
/// this while a replay is running releases the chain the replay is holding down and
/// leaves it reconnecting against a rewind that is no longer in force.
///
/// Returns once the request is away; the reorg itself is watched on a background
/// thread which holds the claim until the node settles. The `reconsiderblock` has to
/// be fire-and-forget because re-activating a chain can reconnect many blocks and far
/// outlast the RPC socket timeout.
///
/// `Ok(Some(anchor_height))` when the request was issued.
pub fn clear_failure_flags(
    coincube_datadir: &CoincubeDirectory,
    config: &coincubed::config::BitcoindConfig,
    identity: &NodeIdentity,
    plan: RevalidationPlan,
) -> Result<Option<i32>, String> {
    let RevalidationPlan::ClearFailureFlags { anchor_height } = plan else {
        return Ok(None);
    };
    let mut operation = match ChainOperation::claim(coincube_datadir, identity) {
        Ok(operation) => operation,
        Err(ClaimRefused::Busy) => {
            return Err(
                "The managed node is already being repaired; wait for that to finish before \
                 starting another."
                    .to_string(),
            )
        }
        Err(ClaimRefused::UnstableIdentity) => {
            return Err(
                "Could not establish the managed node's identity, so a repair cannot be \
                 recorded against it yet. The node is otherwise fine — try again in a moment, \
                 or restart it."
                    .to_string(),
            )
        }
    };
    // Every non-mutating step first — connect, resolve the anchor — so that by the time
    // the sidecar changes at all, the only thing left that can fail is the write itself.
    // Its own connection, since the watching outlives this call.
    let bitcoind = coincubed::BitcoinD::new(config, "rdts_clear_flags".to_string())
        .map_err(|e| format!("Could not reach the managed node: {e}"))?;
    let anchor = bitcoind
        .get_block_hash(anchor_height)
        .ok_or_else(|| format!("no block at the RDTS anchor height {anchor_height}"))?;

    // Clearing the flags can reorg the node onto a branch that forks anywhere above
    // the anchor, which is far deeper than the poller will follow on its own
    // authority. The anchor bounds how far back that can reach, so authorise it:
    // without this every Vault refuses the repaired chain from here on. As in the
    // replay, an authorisation we cannot persist is one a later session will not
    // have, so it gates the repair rather than merely being logged.
    let sanction = SanctionedRollback::at(&anchor, anchor_height, bitcoind.backend_id());
    if !operation.record_floor(sanction.clone()) {
        return Err(
            "could not record the rollback floor, so Vaults could not adopt the repaired \
             chain; refusing to proceed"
                .to_string(),
        );
    }

    let worker_datadir = coincube_datadir.clone();
    // The worker issues the `reconsiderblock` itself, and is spawned before anything
    // touches the node. Mutating first and spawning second leaves a failed spawn
    // holding nothing: the request is away, the claim is dropped, no authorisation is
    // published, and a deep reorg lands on Vaults that will refuse it for the rest of
    // the session — while this function reports success. Nothing here has changed the
    // chain yet, so a spawn that fails simply un-does the claim.
    let spawned = thread::Builder::new()
        .name("rdts-clear-flags".to_string())
        .spawn(move || {
            // From here the chain is ours to change, so the displaced authorisation
            // cannot be restored.
            operation.commit();
            let _operation = operation;
            info!(
                "Clearing inherited block-index rejections from the RDTS anchor \
                 (height {anchor_height}, {anchor}) so this node can follow the most-work chain."
            );
            // Reorging onto a better branch disconnects the current one back to the
            // fork point before connecting the replacement, so there is a window in
            // which the node's tip is at neither. Unlike a replay's rewind that
            // intermediate tip *is* on the path to the final answer rather than one
            // about to be undone — but holding the claim until it settles means no
            // poller has to reason about that, and the authorisation only goes live
            // once there is nothing left in flight.
            //
            // Measured before the request, so "the node moved to something better"
            // can be told from "the node has not started".
            let baseline = active_chainwork(&bitcoind);
            // The waiting variant, so a reorg that completes inside the socket timeout
            // — the common case, since clearing flags usually reconnects little or
            // nothing — is confirmed outright instead of watched for minutes.
            let coincube_datadir = worker_datadir;
            let quick = match bitcoind.reconsider_block(&anchor) {
                Ok(replied) => replied,
                Err(e) => {
                    warn!("reconsiderblock at height {anchor_height} failed: {e}");
                    return;
                }
            };
            if quick {
                info!("The node finished re-activating its best known chain.");
                confirm_and_publish(&coincube_datadir, &sanction.operation_id);
                return;
            }
            let Some(baseline) = baseline else {
                warn!(
                    "could not measure the node's chain work before clearing the flags, so \
                     completion cannot be confirmed here; a later start will"
                );
                return;
            };
            if watch_reorg(&bitcoind, &baseline, RECONSIDER_DEADLINE) {
                confirm_and_publish(&coincube_datadir, &sanction.operation_id);
                return;
            }

            // Not finished inside the window every Vault is parked for. Let them go —
            // nothing is half-done on this path, so a node still working is just a node
            // catching up — but keep watching, because the authorisation still has to
            // be armed when it does finish. Without this the floor would sit pending
            // until the next start even though the reorg completed minutes later, and
            // every poll in between would refuse the repaired chain.
            drop(_operation);
            info!("Still re-activating; released the node and watching for completion.");
            if watch_reorg(&bitcoind, &baseline, CONFIRM_DEADLINE) {
                reclaim_confirm_and_publish(&coincube_datadir, &sanction.operation_id);
            } else {
                warn!(
                    "the node had not confirmably finished re-activating its chain when we \
                     stopped watching; leaving the recorded rollback floor pending for a \
                     later start to confirm rather than authorising one now"
                );
            }
        });
    if let Err(e) = spawned {
        return Err(format!(
            "could not start the thread that performs the repair, so nothing was changed: {e}"
        ));
    }
    Ok(Some(anchor_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that drive the process-wide sanctioned-rollback slot.
    /// Run in parallel they would each observe the others' arming and disarming.
    static SANCTION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Takes that lock and leaves the slot disarmed on the way in and out, so a
    /// failing assertion cannot leak an exception into the rest of the suite.
    struct SanctionSlot(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl SanctionSlot {
        fn claim() -> Self {
            let guard = SANCTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            coincubed::set_sanctioned_rollback(None);
            Self(guard)
        }
    }

    impl Drop for SanctionSlot {
        fn drop(&mut self) {
            coincubed::set_sanctioned_rollback(None);
        }
    }

    fn facts(network: Network, from: NodeFlavor, to: NodeFlavor) -> ChainFacts {
        ChainFacts {
            network,
            previous_flavor: Some(from),
            current_flavor: to,
            // Comfortably past the anchor, so the height gate isn't what's under test.
            blocks: RDTS_ANCHOR_MAINNET + 5_000,
            best_known_height: RDTS_ANCHOR_MAINNET + 5_000,
            rdts_abandoned: false,
            // Unpruned unless a test says otherwise, so the prune floor isn't a
            // hidden variable in the cases that aren't about it.
            prune_state: coincubed::PruneState::NotPruned,
        }
    }

    /// The node follows less work than it knows about — a rejected or headers-only
    /// branch sits `above` blocks higher than its active tip.
    fn stranded_by(mut f: ChainFacts, above: i32) -> ChainFacts {
        f.best_known_height = f.blocks + above;
        f
    }

    #[test]
    fn rdts_is_only_anchored_on_mainnet() {
        assert_eq!(rdts_anchor_height(Network::Bitcoin), Some(961_631));
        assert_eq!(rdts_anchor_height(Network::Regtest), None);
        assert_eq!(rdts_anchor_height(Network::Signet), None);
        assert_eq!(rdts_anchor_height(Network::Testnet4), None);
    }

    #[test]
    fn networks_without_an_anchor_are_skipped() {
        for network in [Network::Regtest, Network::Signet, Network::Testnet4] {
            let f = facts(network, NodeFlavor::Knots, NodeFlavor::Core);
            // Even a stranded node: we have no defensible height to act from.
            let f = stranded_by(f, 10);
            assert_eq!(
                plan(f),
                RevalidationPlan::Skip(SkipReason::NoDeploymentOnNetwork),
            );
        }
    }

    // The runtime gate rules out only a deployment that will never activate.
    #[test]
    fn an_abandoned_deployment_is_skipped() {
        let mut f = stranded_by(
            facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core),
            10,
        );
        f.rdts_abandoned = true;
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::DeploymentAbandoned)
        );
    }

    // Bitcoin Core does not define `reduced_data` at all, so after a Knots -> Core
    // swap the deployment lookup can only ever come back empty. Reading that as
    // "not live" made the entire repair unreachable on the one node that needs it,
    // so absence must fall through to the height gate rather than short-circuit.
    #[test]
    fn a_missing_deployment_entry_does_not_block_the_repair() {
        let f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core);
        assert!(!f.rdts_abandoned, "absence must not be recorded as failure");
        assert_eq!(
            plan(f),
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET
            },
        );
    }

    // As of writing mainnet has not reached 961,632, so this is what every real
    // node answers today. Shipping before activation is what lets the ledger be
    // seeded correctly on every install before divergence is possible at all.
    #[test]
    fn a_chain_below_the_anchor_is_skipped() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core);
        f.blocks = RDTS_ANCHOR_MAINNET;
        f.best_known_height = RDTS_ANCHOR_MAINNET;
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::NothingAboveAnchor)
        );

        // One block into the mandatory-signalling window, divergence is possible.
        f.blocks = RDTS_ANCHOR_MAINNET + 1;
        f.best_known_height = RDTS_ANCHOR_MAINNET + 1;
        assert_eq!(
            plan(f),
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET
            },
        );
    }

    #[test]
    fn knots_to_core_clears_inherited_rejections() {
        assert_eq!(
            plan(facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core)),
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET
            },
        );
    }

    #[test]
    fn a_healthy_core_node_is_left_alone() {
        assert_eq!(
            plan(facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Core)),
            RevalidationPlan::Skip(SkipReason::NothingToClear),
        );
    }

    // The ledger is not load-bearing: a Core node observably trailing a heavier
    // branch gets repaired even when we have no record of a swap at all (installer
    // switch, lost sidecar, datadir moved between machines).
    #[test]
    fn a_stranded_core_node_is_repaired_without_any_ledger() {
        let mut f = stranded_by(
            facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Core),
            10,
        );
        f.previous_flavor = None;
        assert_eq!(
            plan(f),
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET
            },
        );
    }

    // The most important negative case. A Knots node trailing the most-work chain
    // is doing its job — enforcing RDTS against a non-compliant majority. Clearing
    // its flags would re-validate and re-reject the same blocks on every startup.
    #[test]
    fn a_stranded_knots_node_is_never_repaired() {
        let f = stranded_by(
            facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Knots),
            10,
        );
        assert_eq!(plan(f), RevalidationPlan::Skip(SkipReason::NothingToClear));

        // Not even right after a Core -> Knots swap. That swap does trigger work,
        // but it must be a replay under RDTS — never a "repair" that clears the
        // rejections and drags the node back onto the chain the user just opted out
        // of following.
        let f = stranded_by(
            facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots),
            10,
        );
        assert!(matches!(plan(f), RevalidationPlan::ReplayUnderRdts { .. }));
    }

    // The first rejection is the whole point. A Knots node that rejected the block
    // at the start of the mandatory-signalling window sits at exactly the anchor,
    // with a higher branch it refuses to follow. Gating on the active tip alone
    // declined to repair it after a swap to Core.
    #[test]
    fn a_node_parked_exactly_at_the_anchor_is_still_repaired() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core);
        f.blocks = RDTS_ANCHOR_MAINNET;
        f.best_known_height = RDTS_ANCHOR_MAINNET + 1;
        assert_eq!(
            plan(f),
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET
            },
        );
    }

    // Core -> Knots on an unpruned node: rewind all the way to the anchor, which is
    // the deepest point the two flavours must agree on.
    #[test]
    fn core_to_knots_replays_from_the_anchor() {
        let f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots);
        let p = plan(f);
        assert_eq!(
            p,
            RevalidationPlan::ReplayUnderRdts {
                floor_height: RDTS_ANCHOR_MAINNET,
                target_height: RDTS_ANCHOR_MAINNET + 5_000,
            },
        );
        assert!(p.is_full_coverage());
    }

    // Pruning, not the anchor, is what really bounds the replay: we cannot
    // disconnect blocks whose data is gone. The floor must clear `pruneheight` by
    // the safety margin, because pruning advances between our read and the call and
    // disconnecting into pruned data aborts the node outright.
    #[test]
    fn pruning_raises_the_floor_and_makes_coverage_partial() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots);
        let prune_height = RDTS_ANCHOR_MAINNET + 2_000;
        f.prune_state = coincubed::PruneState::Pruned(prune_height);
        let p = plan(f);
        assert_eq!(
            p,
            RevalidationPlan::ReplayUnderRdts {
                floor_height: prune_height + PRUNE_SAFETY_MARGIN,
                target_height: RDTS_ANCHOR_MAINNET + 5_000,
            },
        );
        assert!(
            !p.is_full_coverage(),
            "a floor above the anchor means blocks were pruned and cannot be re-checked"
        );

        // Pruning below the anchor cannot pull the floor down past it — there is
        // nothing to check below the anchor in the first place.
        f.prune_state = coincubed::PruneState::Pruned(RDTS_ANCHOR_MAINNET - 50_000);
        assert_eq!(
            plan(f),
            RevalidationPlan::ReplayUnderRdts {
                floor_height: RDTS_ANCHOR_MAINNET,
                target_height: RDTS_ANCHOR_MAINNET + 5_000,
            },
        );
    }

    // Once pruning has eaten everything above the anchor there is no mechanism —
    // short of re-downloading the chain — that could re-check anything, so we must
    // decline rather than rewind into data we don't have.
    #[test]
    fn a_fully_pruned_window_is_declined() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots);
        f.prune_state = coincubed::PruneState::Pruned(f.blocks);
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::NothingRetainedAboveAnchor),
        );

        // And the margin counts: a floor that lands exactly on the tip is refused,
        // not attempted with a zero-length replay.
        f.prune_state = coincubed::PruneState::Pruned(f.blocks - PRUNE_SAFETY_MARGIN);
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::NothingRetainedAboveAnchor),
        );
    }

    // An unreadable prune height must not read as "unpruned". Conflating the two
    // would skip the prune floor entirely and let the rewind disconnect blocks whose
    // data is gone, which aborts bitcoind rather than returning an error.
    #[test]
    fn an_unreadable_prune_height_declines_rather_than_assuming_unpruned() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots);
        f.prune_state = coincubed::PruneState::PrunedUnknown;
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::PruneHeightUnknown)
        );

        // Contrast: genuinely unpruned still replays from the anchor, so the new
        // state hasn't just made everything decline.
        f.prune_state = coincubed::PruneState::NotPruned;
        assert!(matches!(plan(f), RevalidationPlan::ReplayUnderRdts { .. }));
    }

    // A rewind is only ever justified by an actual swap from Core. Restarting a
    // Knots node must not re-run it every time.
    #[test]
    fn a_restarting_knots_node_does_not_replay() {
        let f = stranded_by(
            facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Knots),
            10,
        );
        assert_eq!(plan(f), RevalidationPlan::Skip(SkipReason::NothingToClear));

        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Knots);
        f.previous_flavor = None;
        assert_eq!(plan(f), RevalidationPlan::Skip(SkipReason::NothingToClear));
    }

    #[test]
    fn the_rewind_record_survives_a_flavour_write_and_vice_versa() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-rewind-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let datadir = CoincubeDirectory::new(dir.clone());

        let rewind = RewindInFlight {
            invalidated_hash: "00".repeat(32),
            floor_height: RDTS_ANCHOR_MAINNET,
            target_height: RDTS_ANCHOR_MAINNET + 100,
        };
        assert!(ManagedNodeState::set_rewind(&datadir, Some(rewind.clone())));
        ManagedNodeState::record_run(&datadir, NodeFlavor::Knots);

        // Recording the flavour must not drop the rewind: losing it would strand the
        // node below the invalidated block with nothing left to undo it.
        let loaded = ManagedNodeState::load(&datadir);
        assert_eq!(loaded.rewind, Some(rewind));
        assert_eq!(loaded.last_run_flavor, Some(NodeFlavor::Knots));

        // And clearing the rewind must not drop the flavour.
        assert!(ManagedNodeState::set_rewind(&datadir, None));
        let loaded = ManagedNodeState::load(&datadir);
        assert_eq!(loaded.rewind, None);
        assert_eq!(loaded.last_run_flavor, Some(NodeFlavor::Knots));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Completing a replay has to retire the rewind and advance the flavour together.
    // Two writes leave a window whose crash outcome is the worst of both: no rewind
    // record, so nothing knows one ever happened, and the flavour still Core, so the
    // next start plans the entire hours-long replay again.
    #[test]
    fn a_completed_rewind_retires_with_the_flavour_in_one_write() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-finish-rewind-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let datadir = CoincubeDirectory::new(dir.clone());

        ManagedNodeState::record_run(&datadir, NodeFlavor::Core);
        assert!(ManagedNodeState::set_rewind(
            &datadir,
            Some(RewindInFlight {
                invalidated_hash: "00".repeat(32),
                floor_height: RDTS_ANCHOR_MAINNET,
                target_height: RDTS_ANCHOR_MAINNET + 100,
            })
        ));

        assert!(ManagedNodeState::finish_rewind(&datadir, NodeFlavor::Knots));
        let loaded = ManagedNodeState::load(&datadir);
        assert_eq!(loaded.rewind, None);
        assert_eq!(loaded.last_run_flavor, Some(NodeFlavor::Knots));

        // Unreadable state must not be papered over with a default: writing back a
        // fabricated one would erase a rewind that is still in flight.
        std::fs::write(ManagedNodeState::path(&datadir), b"{ not json").unwrap();
        assert!(!ManagedNodeState::finish_rewind(
            &datadir,
            NodeFlavor::Knots
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A floor for a repair that has been seen to finish, i.e. one that may be armed.
    fn a_floor() -> SanctionedRollback {
        SanctionedRollback {
            confirmed: true,
            ..a_pending_floor()
        }
    }

    /// The same floor as it is first written: recorded, but not yet seen to finish.
    fn a_pending_floor() -> SanctionedRollback {
        SanctionedRollback {
            hash: "00".repeat(31) + "2a",
            height: RDTS_ANCHOR_MAINNET,
            node_addr: "127.0.0.1:8332".to_string(),
            node_credentials: coincubed::BackendId::fingerprint("cookie:/managed/.cookie"),
            confirmed: false,
            operation_id: "operation-A".to_string(),
        }
    }

    fn a_node_id() -> coincubed::BackendId {
        coincubed::BackendId {
            addr: "127.0.0.1:8332".parse().unwrap(),
            credentials: coincubed::BackendId::fingerprint("cookie:/managed/.cookie"),
        }
    }

    // The floor of a repair has to outlive the session that performed it: the Vault
    // that must adopt the rollback may not even be open yet, and without the record
    // its poller refuses the repaired chain as an implausible reorg forever.
    #[test]
    fn a_repairs_rollback_floor_is_persisted_and_published() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-sanction-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let datadir = CoincubeDirectory::new(dir.clone());
        let _slot = SanctionSlot::claim();

        let floor = a_floor();
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(floor.clone())
        ));

        // Durable...
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(floor.clone())
        );
        // ...but NOT yet live. Recording happens before the chain is touched; a
        // poll already in flight must not be able to adopt the transient rewind.
        assert!(coincubed::sanctioned_rollback().is_none());

        // Publishing is the separate, later step, once the chain is final.
        publish_sanctioned_rollback(Some(&floor));
        let published = coincubed::sanctioned_rollback().expect("published");
        assert_eq!(published.floor.height, RDTS_ANCHOR_MAINNET);
        assert_eq!(published.floor.hash.to_string(), floor.hash);
        assert_eq!(published.node.addr.to_string(), floor.node_addr);
        assert_eq!(published.node.credentials, floor.node_credentials);

        // Republishing from a fresh load is what re-arms it on the next start.
        coincubed::set_sanctioned_rollback(None);
        publish_sanctioned_rollback(
            ManagedNodeState::load(&datadir)
                .sanctioned_rollback
                .as_ref(),
        );
        assert!(coincubed::sanctioned_rollback().is_some());

        // An unreadable hash, or an unreadable node address, disarms rather than
        // authorising something arbitrary.
        publish_sanctioned_rollback(Some(&SanctionedRollback {
            hash: "not a block hash".to_string(),
            ..a_floor()
        }));
        assert!(coincubed::sanctioned_rollback().is_none());
        publish_sanctioned_rollback(Some(&SanctionedRollback {
            node_addr: "not an address".to_string(),
            ..a_floor()
        }));
        assert!(coincubed::sanctioned_rollback().is_none());

        // A record from before the identity was scoped to credentials matches on
        // address alone, which any bitcoind on that port would satisfy. Dropped, not
        // widened.
        publish_sanctioned_rollback(Some(&SanctionedRollback {
            node_credentials: String::new(),
            ..a_floor()
        }));
        assert!(coincubed::sanctioned_rollback().is_none());

        // A floor that could not be written must report failure, and must not leave a
        // live in-memory exception behind either: the caller aborts the repair on
        // this, and an authorisation this process alone can see is exactly the state
        // that strands the next one.
        std::fs::write(ManagedNodeState::path(&datadir), b"{ not json").unwrap();
        assert!(!ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(floor.clone())
        ));
        assert!(coincubed::sanctioned_rollback().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Only a positive "nothing is pending" lets planning run. Both other answers mean
    // the height the node is offering may describe a half-rewound chain — and planning
    // from that is what retires the swap the recovery has not finished yet.
    #[test]
    fn only_an_idle_recovery_permits_planning() {
        assert!(RecoveryState::Idle.may_plan());
        assert!(!RecoveryState::InFlight.may_plan());
        assert!(!RecoveryState::Indeterminate.may_plan());
    }

    // A recorded floor alongside an unfinished rewind describes a chain parked
    // mid-repair, not a repaired one. Re-arming the exception at startup on the
    // strength of the record alone would authorise pollers to adopt a rollback that
    // is about to be undone by the reconnect.
    #[test]
    fn a_saved_floor_is_not_re_armed_while_a_rewind_is_outstanding() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-rearm-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let datadir = CoincubeDirectory::new(dir.clone());
        let _slot = SanctionSlot::claim();

        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(a_floor())
        ));
        assert!(ManagedNodeState::set_rewind(
            &datadir,
            Some(RewindInFlight {
                invalidated_hash: "00".repeat(32),
                floor_height: RDTS_ANCHOR_MAINNET,
                target_height: RDTS_ANCHOR_MAINNET + 5_000,
            })
        ));

        // This is the gate `reconcile_after_start` applies.
        let recorded = ManagedNodeState::load(&datadir);
        assert!(recorded.rewind.is_some());
        if recorded.rewind.is_none() {
            publish_sanctioned_rollback(recorded.sanctioned_rollback.as_ref());
        }
        assert!(coincubed::sanctioned_rollback().is_none());

        // Once the rewind retires, the same record does re-arm.
        assert!(ManagedNodeState::finish_rewind(&datadir, NodeFlavor::Knots));
        let recorded = ManagedNodeState::load(&datadir);
        if recorded.rewind.is_none() {
            publish_sanctioned_rollback(recorded.sanctioned_rollback.as_ref());
        }
        assert!(coincubed::sanctioned_rollback().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn a_temp_datadir(name: &str) -> (std::path::PathBuf, CoincubeDirectory) {
        let dir = std::env::temp_dir().join(format!(
            "coincube-{name}-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.clone(), CoincubeDirectory::new(dir))
    }

    // The second-cycle case. A completed repair leaves a live exception naming this
    // node and this floor; the next replay rewinds to the same floor on the same
    // node, so that stale exception fits the transient rewind exactly. Recording the
    // new floor without publishing it protects nothing while the old one is armed —
    // claiming the node has to empty the slot.
    #[test]
    fn claiming_the_node_withdraws_a_previous_repairs_authorisation() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("claim");

        // A repair completed earlier in this process left its authorisation live.
        publish_sanctioned_rollback(Some(&a_floor()));
        assert!(coincubed::sanctioned_rollback().is_some());

        // Claiming the node for the next one takes it away again, before any
        // `invalidateblock` has had the chance to park the chain at that floor.
        let mut operation =
            ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        assert!(coincubed::sanctioned_rollback().is_none());

        // And it really is exclusive: a second claim finds the node taken.
        assert!(matches!(
            ChainOperation::claim(&datadir, &NodeIdentity::Stable),
            Err(ClaimRefused::Busy)
        ));

        // Committed, so dropping it leaves the slot empty — the chain has moved and
        // the old authorisation no longer describes anything.
        operation.commit();
        drop(operation);
        assert!(coincubed::sanctioned_rollback().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Withdrawing the previous authorisation is only right once the chain is actually
    // going to move. Everything between claiming the node and the first mutating call
    // can fail — connecting, reading the anchor, persisting the floor, spawning the
    // worker — and on those paths the earlier repair is still exactly as valid as it
    // was. Leaving it withdrawn strands a Vault that had not adopted it yet, for the
    // rest of the session, over an operation that never started.
    #[test]
    fn an_abandoned_claim_restores_what_it_displaced() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("abandoned-claim");

        let earlier = a_floor();
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(earlier.clone())
        ));
        publish_sanctioned_rollback(Some(&earlier));

        // A repair claims the node, records its own floor over the earlier one, and
        // then gives up before touching the chain.
        {
            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            assert!(coincubed::sanctioned_rollback().is_none());
            let next = SanctionedRollback {
                height: RDTS_ANCHOR_MAINNET + 4_000,
                ..a_floor()
            };
            assert!(operation.record_floor(next.clone()));
            assert_eq!(
                ManagedNodeState::load(&datadir).sanctioned_rollback,
                Some(next)
            );
        }

        // Both the live exception and the durable record are back as they were.
        assert_eq!(
            coincubed::sanctioned_rollback().map(|s| s.floor.height),
            Some(earlier.height)
        );
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(earlier)
        );
        // And the node is free for the next attempt.
        assert!(ChainOperation::claim(&datadir, &NodeIdentity::Stable).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A floor is written before the chain is touched, so on its own it only says a
    // repair was *started*. Arming it on a restart would let a poller adopt a chain
    // that may still be mid-reorg — the record has to distinguish pending from
    // confirmed, and publication has to refuse the former wherever it comes from.
    #[test]
    fn a_pending_repair_is_never_armed() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("pending");

        // What `clear_failure_flags` leaves on disk before the reorg is confirmed.
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(a_pending_floor())
        ));

        // The startup gate: a pending floor is not confirmed, so it is not armed.
        let recorded = ManagedNodeState::load(&datadir);
        let floor = recorded.sanctioned_rollback.as_ref().expect("recorded");
        assert!(!floor.confirmed);
        publish_sanctioned_rollback(Some(floor));
        assert!(
            coincubed::sanctioned_rollback().is_none(),
            "a repair that was never seen to finish must not authorise anything"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ...and the other half: a repair that finishes later — after the window every
    // Vault was parked for, or in a later session entirely — still has to become
    // armable, or the Vaults that needed it refuse the repaired chain forever.
    #[test]
    fn a_repair_confirmed_after_the_deadline_becomes_armable() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("late-confirm");

        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(a_pending_floor())
        ));
        publish_sanctioned_rollback(
            ManagedNodeState::load(&datadir)
                .sanctioned_rollback
                .as_ref(),
        );
        assert!(coincubed::sanctioned_rollback().is_none());

        // The reorg is observed to finish. This is the single route from pending to
        // armed, and it both records the fact and hands back what to publish.
        let confirmed = ManagedNodeState::confirm_sanctioned_rollback(
            &datadir,
            &a_pending_floor().operation_id,
        )
        .expect("confirmed");
        assert!(confirmed.confirmed);
        publish_sanctioned_rollback(Some(&confirmed));
        assert!(coincubed::sanctioned_rollback().is_some());

        // Durably confirmed, so a later start arms it without re-confirming.
        coincubed::set_sanctioned_rollback(None);
        let reloaded = ManagedNodeState::load(&datadir);
        assert!(reloaded.sanctioned_rollback.as_ref().unwrap().confirmed);
        publish_sanctioned_rollback(reloaded.sanctioned_rollback.as_ref());
        assert!(coincubed::sanctioned_rollback().is_some());

        // Confirming with nothing recorded is not an error, just nothing to do.
        assert!(ManagedNodeState::write_sanctioned_rollback(&datadir, None));
        assert!(ManagedNodeState::confirm_sanctioned_rollback(&datadir, "operation-A").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A `BitcoinD` caches the identity it derives at construction, so one built while the
    // datadir's marker is merely *late* reports something that changes the moment the
    // marker lands. A repair recorded in that window names an identity no later client
    // agrees with. The claim is the single gate every chain operation passes through, so
    // requiring a settled identity there is what makes "no repair without one" hold by
    // construction rather than by each caller remembering.
    #[test]
    fn an_unsettled_identity_permits_no_chain_operation() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("unstable-identity");
        let config = coincubed::config::BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(dir.join(".cookie")),
            addr: "127.0.0.1:8332".parse().unwrap(),
        };

        assert!(matches!(
            ChainOperation::claim(&datadir, &NodeIdentity::Unstable),
            Err(ClaimRefused::UnstableIdentity)
        ));

        // The manual repair refuses too, and refuses *before* contacting the node or
        // writing anything — this test has no node to contact, which is the proof.
        let refusal = clear_failure_flags(
            &datadir,
            &config,
            &NodeIdentity::Unstable,
            RevalidationPlan::ClearFailureFlags {
                anchor_height: RDTS_ANCHOR_MAINNET,
            },
        )
        .expect_err("must refuse");
        assert!(
            refusal.contains("identity"),
            "the refusal should say why: {}",
            refusal
        );
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            None,
            "a refused repair must leave no half-recorded floor behind"
        );

        // A plan that calls for no repair is still a no-op rather than a failure, so an
        // unsettled identity does not turn ordinary starts into errors.
        assert_eq!(
            clear_failure_flags(
                &datadir,
                &config,
                &NodeIdentity::Unstable,
                RevalidationPlan::Skip(SkipReason::NothingToClear)
            ),
            Ok(None)
        );

        // And the gate is not simply always-closed: once the identity is settled the same
        // claim succeeds.
        assert!(ChainOperation::claim(&datadir, &NodeIdentity::Stable).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The flag-clearing path releases the node after a bounded window and keeps watching
    // for hours. In that time another repair can claim the node, withdraw the live
    // authorisation and start rewinding the chain — so when the old watcher finally has
    // something to confirm, the record it finds may not be its own. Confirming by
    // position would mark the newer, possibly mid-rewind operation as finished and arm an
    // exception over exactly the transient the claim exists to keep pollers away from.
    #[test]
    fn a_stale_watcher_cannot_confirm_a_newer_repair() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("stale-watcher");

        // Repair A records its floor and, in the real thing, later drops its claim and
        // carries on watching.
        let a = a_pending_floor();
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(a.clone())
        ));

        // Repair B claims the node, withdrawing the live authorisation, and replaces the
        // durable floor with its own.
        let mut b_operation =
            ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        let b = SanctionedRollback {
            operation_id: "operation-B".to_string(),
            height: RDTS_ANCHOR_MAINNET + 2_000,
            ..a_pending_floor()
        };
        assert!(b_operation.record_floor(b.clone()));
        b_operation.commit();
        assert!(coincubed::sanctioned_rollback().is_none());

        // A's watcher fires. It cannot even take the claim, because B owns the node.
        reclaim_confirm_and_publish(&datadir, &a.operation_id);
        assert!(
            coincubed::sanctioned_rollback().is_none(),
            "a stale watcher armed an exception while another operation held the node"
        );
        // B is untouched: still pending, still B's.
        let stored = ManagedNodeState::load(&datadir)
            .sanctioned_rollback
            .expect("kept");
        assert!(!stored.confirmed, "B was marked finished by A's watcher");
        assert_eq!(stored.operation_id, b.operation_id);

        // Now B finishes and gives the node up. A's watcher fires again — it can take
        // the claim this time, so only the operation id stands between it and arming
        // something that is not its own.
        drop(b_operation);
        reclaim_confirm_and_publish(&datadir, &a.operation_id);
        assert!(
            coincubed::sanctioned_rollback().is_none(),
            "a stale watcher confirmed a record belonging to another operation"
        );
        let stored = ManagedNodeState::load(&datadir)
            .sanctioned_rollback
            .expect("kept");
        assert!(!stored.confirmed);
        assert_eq!(stored.operation_id, b.operation_id);

        // And B confirming its own record still works, so the id check has not simply
        // wedged confirmation.
        reclaim_confirm_and_publish(&datadir, &b.operation_id);
        assert_eq!(
            coincubed::sanctioned_rollback().map(|s| s.floor.height),
            Some(b.height)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The day a datadir gains an instance marker, every authorisation recorded before
    // it changes fingerprint. Left alone they would all stop matching at once, and
    // nothing schedules another repair — so an authorisation provably written by this
    // same node under the older scheme is re-stamped instead.
    #[test]
    fn a_path_only_authorisation_survives_the_marker_being_introduced() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("restamp");

        // The identity before and after the datadir was stamped.
        let legacy = a_node_id();
        let stamped = coincubed::BackendId {
            addr: legacy.addr,
            credentials: coincubed::BackendId::fingerprint(
                "cookie:/managed/.cookie|instance:abc123",
            ),
        };
        assert_ne!(legacy.credentials, stamped.credentials);

        // Recorded under the old scheme, and confirmed, so the only thing standing
        // between it and being armed is the identity.
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(a_floor())
        ));
        ManagedNodeState::migrate_sanctioned_rollback(&datadir, &stamped, &legacy);
        let migrated = ManagedNodeState::load(&datadir)
            .sanctioned_rollback
            .expect("kept");
        assert_eq!(migrated.node_credentials, stamped.credentials);
        // Still confirmed: re-stamping the identity must not reset what we knew about
        // the repair's completion.
        assert!(migrated.confirmed);

        // A record matching neither the current nor the previous fingerprint is not
        // ours to re-stamp, and is left exactly as it is.
        let foreign = SanctionedRollback {
            node_credentials: coincubed::BackendId::fingerprint("cookie:/elsewhere/.cookie"),
            ..a_floor()
        };
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(foreign.clone())
        ));
        ManagedNodeState::migrate_sanctioned_rollback(&datadir, &stamped, &legacy);
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(foreign)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A record written before repairs were scoped to a node names only an address,
    // which any bitcoind on that port would match. It cannot be published as-is — but
    // discarding it is worse: nothing would schedule another repair (a Knots → Knots
    // restart plans nothing), so a Vault still holding the pre-repair chain would
    // refuse the node's chain for good. Migration is the only answer that neither
    // widens the exception nor throws it away.
    #[test]
    fn a_legacy_record_is_migrated_rather_than_dropped() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("migrate");

        let legacy = SanctionedRollback {
            node_credentials: String::new(),
            ..a_floor()
        };
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(legacy.clone())
        ));
        // As it stands it authorises nothing, which is why it must not be left alone.
        publish_sanctioned_rollback(Some(&legacy));
        assert!(coincubed::sanctioned_rollback().is_none());

        // The managed node we are actually talking to lends it an identity.
        let node = a_node_id();
        ManagedNodeState::migrate_sanctioned_rollback(&datadir, &node, &node);
        let migrated = ManagedNodeState::load(&datadir)
            .sanctioned_rollback
            .expect("kept");
        assert_eq!(migrated.node_credentials, node.credentials);
        assert_eq!(migrated.height, legacy.height);
        publish_sanctioned_rollback(Some(&migrated));
        assert_eq!(
            coincubed::sanctioned_rollback().map(|s| s.node),
            Some(node.clone())
        );

        // A record naming a different endpoint carries no authority for this node, so
        // that one really is dropped — and dropped from disk, so it stops being
        // reconsidered on every start.
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(SanctionedRollback {
                node_addr: "127.0.0.1:18332".to_string(),
                node_credentials: String::new(),
                ..a_floor()
            })
        ));
        ManagedNodeState::migrate_sanctioned_rollback(&datadir, &node, &node);
        assert_eq!(ManagedNodeState::load(&datadir).sanctioned_rollback, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The manual "Re-check chain" action is what we tell the user to reach for when the
    // state file cannot be read, so it has to be able to get past one — and the way it
    // does so has to be crash-safe at every step. The canonical path must never be
    // durably absent: a crash while it is missing reads as "nothing pending" to the next
    // start, from a node that may be parked below an invalidated block.
    #[test]
    fn replacing_an_unreadable_state_file_is_crash_safe_at_every_stage() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("crash-safe-replace");
        let canonical = ManagedNodeState::path(&datadir);
        let unreadable = b"{ not json".as_slice();
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, unreadable).unwrap();

        // Stage 1 — evidence is taken as a copy, so the canonical file is untouched.
        // A crash here leaves the original in place and reconciliation fails closed.
        let evidence = write_evidence_copy(&canonical, unreadable).expect("evidence");
        assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);
        assert_eq!(std::fs::read(&evidence).unwrap(), unreadable);
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        std::fs::remove_file(&evidence).unwrap();

        // Stage 2 — the whole transition. Before it, the canonical file is the original;
        // after it, a valid pending record. There is no step in between at which the
        // canonical path does not exist.
        let (evidence, original) =
            match ManagedNodeState::replace_unreadable(&datadir, a_pending_floor()) {
                PendingInstall::Installed { evidence, original } => (evidence, original),
                other => panic!("expected a durable install, got {:?}", other),
            };
        assert_eq!(original, unreadable);
        assert_eq!(std::fs::read(&evidence).unwrap(), unreadable);
        assert!(canonical.exists(), "the canonical path went missing");
        let loaded = ManagedNodeState::load(&datadir);
        assert_eq!(loaded.sanctioned_rollback, Some(a_pending_floor()));
        assert!(
            !loaded.sanctioned_rollback.unwrap().confirmed,
            "the installed record must be pending, so a later start resumes it"
        );

        // Stage 3 — undoing it goes back the same way, and takes the evidence for a
        // repair that never happened with it.
        write_atomically(&canonical, &original).unwrap();
        std::fs::remove_file(&evidence).unwrap();
        assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Arms a failpoint for the current thread and disarms it on drop, so a failing
    /// assertion cannot leak an injected failure into the rest of the test.
    struct Injected;

    impl Injected {
        fn at(point: Failpoint) -> Self {
            Self::all(&[point])
        }

        fn all(points: &[Failpoint]) -> Self {
            ARMED_FAILPOINTS.with(|armed| *armed.borrow_mut() = points.to_vec());
            Self
        }
    }

    impl Drop for Injected {
        fn drop(&mut self) {
            ARMED_FAILPOINTS.with(|armed| armed.borrow_mut().clear());
        }
    }

    /// A datadir holding an unreadable canonical sidecar, plus those bytes.
    fn an_unreadable_sidecar(name: &str) -> (std::path::PathBuf, CoincubeDirectory, Vec<u8>) {
        let (dir, datadir) = a_temp_datadir(name);
        let canonical = ManagedNodeState::path(&datadir);
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        let unreadable = b"{ not json".to_vec();
        std::fs::write(&canonical, &unreadable).unwrap();
        (dir, datadir, unreadable)
    }

    // Every way the evidence copy can fail after its name has been reserved. The name is
    // finite and shared, so an empty or half-written file left behind is worse than the
    // failure itself: it reads as completed evidence and burns one of the names, and
    // enough of them make manual repair impossible.
    #[test]
    fn a_failed_evidence_copy_leaves_no_partial_artifact() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir, unreadable) = an_unreadable_sidecar("evidence-failpoints");
        let canonical = ManagedNodeState::path(&datadir);
        let first = canonical.with_extension("corrupt");

        for point in [
            Failpoint::EvidenceWrite,
            Failpoint::EvidenceSync,
            Failpoint::EvidenceDirSync,
        ] {
            let armed = Injected::at(point);
            let outcome = ManagedNodeState::replace_unreadable(&datadir, a_pending_floor());
            drop(armed);

            assert!(
                matches!(outcome, PendingInstall::NotInstalled(_)),
                "{:?} should stop before the canonical file changes, got {:?}",
                point,
                outcome
            );
            // The reserved name is free again, not holding a partial file...
            assert!(
                !first.exists(),
                "{:?} left a partial evidence file behind",
                point
            );
            assert_eq!(corrupt_evidence(&datadir), Vec::<std::path::PathBuf>::new());
            // ...and the canonical state is byte-identical and still fail-closed.
            assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);
            assert!(ManagedNodeState::try_load(&datadir).is_err());
        }

        // Cleanup failing is reported rather than hidden: presenting a partial file as a
        // completed quarantine copy is the one outcome that must not happen quietly.
        {
            let _armed = Injected::all(&[Failpoint::EvidenceWrite, Failpoint::EvidenceCleanup]);
            let error = match write_evidence_copy(&canonical, &unreadable) {
                Err(e) => e.to_string(),
                Ok(path) => panic!("expected cleanup to fail, got evidence at {:?}", path),
            };
            assert!(
                error.contains("could not be removed"),
                "the cleanup failure should be explicit: {}",
                error
            );
        }
        // That one really is still there, which is why it was reported.
        assert!(first.exists());
        std::fs::remove_file(&first).unwrap();

        // And once the filesystem behaves, the same name is allocated and the repair goes
        // through — the failures burned nothing.
        let (evidence, _) = match ManagedNodeState::replace_unreadable(&datadir, a_pending_floor())
        {
            PendingInstall::Installed { evidence, original } => (evidence, original),
            other => panic!("expected a healthy retry to install, got {:?}", other),
        };
        assert_eq!(evidence, first);
        assert_eq!(std::fs::read(&evidence).unwrap(), unreadable);
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(a_pending_floor())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The readable-sidecar path had the same post-rename blind spot as the unreadable one:
    // a directory-fsync failure reports "not written" over a canonical file that already
    // holds the new floor. Believing that meant the displaced record was never put back —
    // so a confirmed authorisation Vaults were relying on got replaced by a pending repair
    // that never started, which the next start would then resume.
    #[test]
    fn an_undurable_floor_write_still_restores_what_it_displaced() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("undurable-readable");

        // 1. A readable sidecar holding a confirmed repair, alongside state that has
        //    nothing to do with repairs and must survive untouched.
        let previous = a_floor();
        assert!(previous.confirmed);
        ManagedNodeState::record_run(&datadir, NodeFlavor::Knots);
        let rewind = RewindInFlight {
            invalidated_hash: "11".repeat(32),
            floor_height: RDTS_ANCHOR_MAINNET,
            target_height: RDTS_ANCHOR_MAINNET + 7,
        };
        assert!(ManagedNodeState::set_rewind(&datadir, Some(rewind.clone())));
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(previous.clone())
        ));

        // 2. It is armed, which is the state a Vault's poller is relying on.
        publish_sanctioned_rollback(Some(&previous));
        assert!(coincubed::sanctioned_rollback().is_some());

        // 3. A new operation claims the node, withdrawing that authorisation.
        let mut operation =
            ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        assert!(coincubed::sanctioned_rollback().is_none());

        // 4 & 5. Its floor lands but cannot be confirmed durable, so it is refused — the
        //        caller stops before `reconsiderblock`.
        let pending = SanctionedRollback {
            operation_id: "operation-late".to_string(),
            height: RDTS_ANCHOR_MAINNET + 900,
            ..a_pending_floor()
        };
        let armed = Injected::at(Failpoint::StateDirSync);
        let recorded = operation.record_floor(pending.clone());
        drop(armed);
        assert!(!recorded, "an undurable write must not report success");

        // 6. And yet the canonical file really does name the new pending floor — which is
        //    exactly why "not written" cannot be taken at face value.
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(pending.clone())
        );

        // 7. Abandoned without ever reaching the chain.
        drop(operation);

        // 8, 9, 10. The previous confirmed record is back on disk, the previous
        //           authorisation is armed again, and the restoration is durable rather
        //           than only true for this process.
        let reloaded = ManagedNodeState::load(&datadir);
        assert_eq!(reloaded.sanctioned_rollback, Some(previous.clone()));
        assert_eq!(
            coincubed::sanctioned_rollback().map(|s| s.floor.height),
            Some(previous.height)
        );
        // Unrelated state came through untouched.
        assert_eq!(reloaded.rewind, Some(rewind));
        assert_eq!(reloaded.last_run_flavor, Some(NodeFlavor::Knots));

        // 11. The abandoned operation is neither armed nor resumable: what is on disk is
        //     the old confirmed floor, so startup arms it rather than resuming anything.
        let stored = reloaded.sanctioned_rollback.unwrap();
        assert_ne!(stored.operation_id, pending.operation_id);
        assert!(stored.confirmed, "startup would resume a pending floor");
        assert!(
            ManagedNodeState::confirm_sanctioned_rollback(&datadir, &pending.operation_id)
                .is_none(),
            "the abandoned operation must not be confirmable"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // What the restoration itself can run into. None of it can leave the caller believing
    // something happened that did not, or the reverse.
    #[test]
    fn floor_restoration_reports_which_side_of_the_rename_it_failed_on() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("restore-failures");
        let previous = a_floor();
        let pending = SanctionedRollback {
            operation_id: "operation-new".to_string(),
            ..a_pending_floor()
        };

        // A write that never lands is reported as such, and the canonical file agrees:
        // the read-back still names the *old* operation, so it is not mistaken for a
        // replacement that merely lacked its fsync.
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(previous.clone())
        ));
        let armed = Injected::at(Failpoint::StateRename);
        let outcome =
            ManagedNodeState::write_sanctioned_rollback_tracked(&datadir, Some(pending.clone()));
        drop(armed);
        assert!(
            matches!(outcome, FloorWrite::NotWritten(_)),
            "a read-back naming the old operation means nothing was replaced, got {:?}",
            outcome
        );
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(previous.clone())
        );

        // Failing *before* the restoration rename leaves the new floor in place, and says
        // so — the sidecar names a repair that never started, and a later start resumes it
        // rather than silently doing nothing.
        {
            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            assert!(operation.record_floor(pending.clone()));
            let _armed = Injected::at(Failpoint::StateRename);
            drop(operation);
        }
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(pending.clone()),
            "a restoration that never landed must not look like it did"
        );

        // Failing *after* the restoration rename but before its fsync still restores: the
        // canonical file holds the previous record, durability alone is unconfirmed.
        assert!(ManagedNodeState::write_sanctioned_rollback(
            &datadir,
            Some(previous.clone())
        ));
        {
            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            assert!(operation.record_floor(pending.clone()));
            let _armed = Injected::at(Failpoint::StateDirSync);
            drop(operation);
        }
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(previous),
            "the previous record should be back even when its fsync failed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The rename lands before the directory entry is flushed, so a failure at the last
    // step returns an error over a canonical file that already holds the pending record.
    // Reading that as "nothing happened" deletes the evidence for a repair that startup
    // will go on to resume — and leaves the operation unable to roll itself back.
    #[test]
    fn a_post_replacement_failure_is_not_mistaken_for_nothing_happening() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir, unreadable) = an_unreadable_sidecar("post-replacement");
        let canonical = ManagedNodeState::path(&datadir);

        // Fail after the rename but before durability is confirmed.
        let armed = Injected::at(Failpoint::StateDirSync);
        let outcome = ManagedNodeState::replace_unreadable(&datadir, a_pending_floor());
        drop(armed);

        let (evidence, original) = match outcome {
            PendingInstall::InstalledNotDurable {
                evidence, original, ..
            } => (evidence, original),
            other => panic!(
                "expected an installed-but-undurable outcome, got {:?}",
                other
            ),
        };
        // The outcome and the filesystem agree: the record really is installed...
        assert_eq!(
            ManagedNodeState::load(&datadir).sanctioned_rollback,
            Some(a_pending_floor())
        );
        // ...and the evidence is still there, because it is now the only copy of what the
        // repair displaced.
        assert!(evidence.exists(), "the evidence copy was deleted");
        assert_eq!(std::fs::read(&evidence).unwrap(), unreadable);
        assert_eq!(original, unreadable);

        // Restore for the next part of the test.
        write_atomically(&canonical, &unreadable).unwrap();
        std::fs::remove_file(&evidence).unwrap();

        // Through the operation: the same failure is remembered, so the rollback stays
        // correct — but reported as failure, so the caller never reaches the RPC. An
        // intent we could not confirm durable is one that may not survive to be resumed.
        {
            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            let armed = Injected::at(Failpoint::StateDirSync);
            let recorded = operation.record_floor(a_pending_floor());
            drop(armed);
            assert!(!recorded, "an undurable write must not report success");
            assert_eq!(
                ManagedNodeState::load(&datadir).sanctioned_rollback,
                Some(a_pending_floor()),
                "the canonical file should reflect what actually happened"
            );
            // No commit, so dropping rolls it back — which only works because the
            // operation remembered the replacement despite reporting failure.
        }
        assert!(canonical.exists(), "the canonical path went missing");
        assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        assert_eq!(corrupt_evidence(&datadir), Vec::<std::path::PathBuf>::new());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The failures on the other side of the rename, where the canonical file genuinely is
    // untouched. Outcome and filesystem have to agree here too, in the opposite direction.
    #[test]
    fn a_pre_replacement_failure_leaves_the_canonical_state_untouched() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir, unreadable) = an_unreadable_sidecar("pre-replacement");
        let canonical = ManagedNodeState::path(&datadir);

        for point in [
            Failpoint::StateWrite,
            Failpoint::StateSync,
            Failpoint::StateRename,
        ] {
            let armed = Injected::at(point);
            let outcome = ManagedNodeState::replace_unreadable(&datadir, a_pending_floor());
            drop(armed);

            assert!(
                matches!(outcome, PendingInstall::NotInstalled(_)),
                "{:?} should report that nothing was replaced, got {:?}",
                point,
                outcome
            );
            assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);
            assert!(ManagedNodeState::try_load(&datadir).is_err());
            // Nothing was installed, so the evidence for it is gone too...
            assert_eq!(corrupt_evidence(&datadir), Vec::<std::path::PathBuf>::new());
            // ...and no staging file is left for the next writer to trip over.
            assert!(!canonical.with_extension("tmp").exists());
        }

        // And through the operation, which is what gates the mutating RPC.
        let mut operation =
            ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        let armed = Injected::at(Failpoint::StateRename);
        assert!(!operation.record_floor(a_pending_floor()));
        drop(armed);
        drop(operation);
        assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Evidence names are reserved with `create_new`, so two repairs racing — here, or in
    // another process, since the maintenance claim is only process-wide — cannot be
    // handed the same name or overwrite each other's evidence.
    #[test]
    fn evidence_names_are_allocated_without_collision() {
        let (dir, datadir) = a_temp_datadir("evidence-names");
        let canonical = ManagedNodeState::path(&datadir);
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();

        let racers = evidence_name_limit() as usize;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(racers));
        let claimed: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..racers)
                .map(|n| {
                    let barrier = barrier.clone();
                    let canonical = canonical.clone();
                    scope.spawn(move || {
                        let body = format!("racer-{n}");
                        barrier.wait();
                        let path = write_evidence_copy(&canonical, body.as_bytes())
                            .expect("evidence name");
                        // Each racer's own bytes, not another's.
                        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
                        path
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let distinct: std::collections::HashSet<_> = claimed.iter().collect();
        assert_eq!(
            distinct.len(),
            racers,
            "two racers were handed the same name"
        );

        // Exhausting the names is an error, not an overwrite — and it happens before
        // anything else is touched.
        assert!(write_evidence_copy(&canonical, b"one too many").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A repair that cannot keep its evidence must not replace the canonical file either,
    // and must not go on to touch the node.
    #[test]
    fn evidence_that_cannot_be_written_leaves_everything_alone() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("evidence-fails");
        let canonical = ManagedNodeState::path(&datadir);
        let unreadable = b"{ not json".as_slice();
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, unreadable).unwrap();

        // Every evidence name is taken, so the copy cannot be made.
        for n in 0..evidence_name_limit() {
            let taken = match n {
                0 => canonical.with_extension("corrupt"),
                n => canonical.with_extension(format!("corrupt.{n}")),
            };
            std::fs::write(&taken, b"someone else's evidence").unwrap();
        }

        assert!(matches!(
            ManagedNodeState::replace_unreadable(&datadir, a_pending_floor()),
            PendingInstall::NotInstalled(_)
        ));
        assert_eq!(
            std::fs::read(&canonical).unwrap(),
            unreadable,
            "the canonical file was replaced despite the evidence failing"
        );
        // And the same through the operation, which is what gates the mutating RPC: a
        // floor it could not record means `clear_failure_flags` returns before spawning
        // the worker that issues `reconsiderblock`.
        let mut operation =
            ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        assert!(!operation.record_floor(a_pending_floor()));
        drop(operation);
        assert_eq!(std::fs::read(&canonical).unwrap(), unreadable);
        assert!(ManagedNodeState::try_load(&datadir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A config pointing at a datadir with no cookie file, so `BitcoinD::new` fails on
    /// A config pointing at a datadir with no cookie file, so `BitcoinD::new` fails on
    /// the spot — a preflight failure *after* the claim, with no node and no network.
    fn an_unreachable_config(dir: &std::path::Path) -> coincubed::config::BitcoindConfig {
        coincubed::config::BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(dir.join(".cookie")),
            addr: "127.0.0.1:1".parse().unwrap(),
        }
    }

    fn corrupt_evidence(datadir: &CoincubeDirectory) -> Vec<std::path::PathBuf> {
        let canonical = ManagedNodeState::path(datadir);
        (0..8)
            .map(|n| match n {
                0 => canonical.with_extension("corrupt"),
                n => canonical.with_extension(format!("corrupt.{n}")),
            })
            .filter(|p| p.exists())
            .collect()
    }

    // Quarantining the unreadable sidecar is only safe if the repair then happens. Moving
    // it and *then* refusing turns a state the next start correctly reads as "something
    // may be pending, do not plan" into "nothing pending" — and it may then plan from a
    // node parked below an invalidated block, which is the failure the record exists to
    // prevent. So it must not be moved before the repair is committed to, and must come
    // back if the repair falls over on the way.
    #[test]
    fn a_refused_repair_leaves_the_recovery_state_where_it_was() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("quarantine-rollback");
        let config = an_unreachable_config(&dir);
        let unreadable = b"{ not json".as_slice();

        let plan = RevalidationPlan::ClearFailureFlags {
            anchor_height: RDTS_ANCHOR_MAINNET,
        };
        let write_unreadable = || {
            std::fs::create_dir_all(ManagedNodeState::path(&datadir).parent().unwrap()).unwrap();
            std::fs::write(ManagedNodeState::path(&datadir), unreadable).unwrap();
        };

        // 1. Refused for an unsettled identity. Nothing is moved, because the refusal
        //    happens before the claim — let alone before any RPC.
        write_unreadable();
        assert!(clear_failure_flags(&datadir, &config, &NodeIdentity::Unstable, plan).is_err());
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        assert_eq!(corrupt_evidence(&datadir), Vec::<std::path::PathBuf>::new());

        // 2. Refused because another chain operation owns the node. Same again.
        let busy = ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
        assert!(clear_failure_flags(&datadir, &config, &NodeIdentity::Stable, plan).is_err());
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        assert_eq!(corrupt_evidence(&datadir), Vec::<std::path::PathBuf>::new());
        drop(busy);

        // 3. Claimed, quarantined, and *then* the node turns out to be unreachable —
        //    the case the ordering alone cannot rule out. The move is undone, so the
        //    next start still sees an unreadable sidecar and declines to plan.
        let refusal = clear_failure_flags(&datadir, &config, &NodeIdentity::Stable, plan)
            .expect_err("unreachable node");
        assert!(
            refusal.contains("Could not reach"),
            "expected a connection failure, got: {}",
            refusal
        );
        assert!(
            ManagedNodeState::try_load(&datadir).is_err(),
            "the unreadable recovery state was not put back"
        );
        assert_eq!(
            std::fs::read(ManagedNodeState::path(&datadir)).unwrap(),
            unreadable,
            "the restored file is not the one that was moved"
        );
        assert_eq!(
            corrupt_evidence(&datadir),
            Vec::<std::path::PathBuf>::new(),
            "a repair that never started left a quarantine artifact behind"
        );
        // And the node is free again, so a later attempt is not blocked by the failed one.
        assert!(ChainOperation::claim(&datadir, &NodeIdentity::Stable).is_ok());

        // 4. The one case the preflight-first ordering cannot rule out: the sidecar has
        //    already been replaced, and the operation is then abandoned without ever
        //    reaching the chain. The original comes back through the same atomic
        //    replacement — so the canonical path is never absent — and the evidence for a
        //    repair that did not happen goes with it.
        let canonical = ManagedNodeState::path(&datadir);
        {
            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            assert!(operation.record_floor(a_pending_floor()));
            // Mid-transition the canonical file holds a valid pending record, not nothing.
            assert_eq!(
                ManagedNodeState::load(&datadir).sanctioned_rollback,
                Some(a_pending_floor())
            );
            assert_eq!(corrupt_evidence(&datadir).len(), 1);
            // ...and no `commit`, so dropping it here rolls the whole thing back.
        }
        assert!(canonical.exists(), "the canonical path went missing");
        assert_eq!(
            std::fs::read(&canonical).unwrap(),
            unreadable,
            "the original unreadable state was not restored byte-for-byte"
        );
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        assert_eq!(
            corrupt_evidence(&datadir),
            Vec::<std::path::PathBuf>::new(),
            "evidence was kept for a repair that never started"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The other half: a repair that *does* reach the chain keeps its quarantine, and each
    // one keeps its own evidence file rather than overwriting the last.
    #[test]
    fn a_committed_repair_keeps_its_quarantine_evidence() {
        let _slot = SanctionSlot::claim();
        let (dir, datadir) = a_temp_datadir("quarantine-commit");

        for expected in ["corrupt", "corrupt.1"] {
            std::fs::create_dir_all(ManagedNodeState::path(&datadir).parent().unwrap()).unwrap();
            std::fs::write(ManagedNodeState::path(&datadir), b"{ not json").unwrap();

            let mut operation =
                ChainOperation::claim(&datadir, &NodeIdentity::Stable).expect("uncontended");
            assert!(operation.record_floor(a_pending_floor()));
            // Past this point the chain has been touched, so the evidence stays where it
            // was put — there is no state to go back to.
            operation.commit();
            drop(operation);

            assert!(
                ManagedNodeState::path(&datadir)
                    .with_extension(expected)
                    .exists(),
                "expected evidence at .{}",
                expected
            );
            // The canonical path holds a valid pending record, so a crash right here
            // leaves the next start something to resume rather than nothing at all.
            let loaded = ManagedNodeState::load(&datadir);
            assert_eq!(loaded.sanctioned_rollback, Some(a_pending_floor()));
        }
        assert_eq!(corrupt_evidence(&datadir).len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn branch(height: i32, branch_len: i32, status: &str) -> coincubed::ChainTipEntry {
        use coincube_core::miniscript::bitcoin::{self, hashes::Hash};
        coincubed::ChainTipEntry {
            height,
            hash: bitcoin::BlockHash::from_byte_array([height as u8; 32]),
            branch_len,
            status: status.to_string(),
        }
    }

    fn candidates(
        tips: &[coincubed::ChainTipEntry],
        blocks: i32,
        floor: i32,
        target: i32,
    ) -> usize {
        candidate_rejected_branches(tips, blocks, floor, target).count()
    }

    // Which branches can be the one we replayed, on `getchaintips` geometry. The
    // shapes that matter are the two the earlier rules each got wrong, plus the one
    // that must never pass.
    #[test]
    fn a_rejected_replay_branch_is_recognised_wherever_the_node_settled() {
        const FLOOR: i32 = 961_631;
        const TARGET: i32 = 970_000;

        // Refused at 966,000, so the Core branch forks at 965,999 and reaches the
        // original tip. The node stopped right there.
        let rejected = branch(TARGET, TARGET - 965_999, "invalid");
        assert_eq!(
            candidates(
                &[branch(965_999, 0, "active"), rejected.clone()],
                965_999,
                FLOOR,
                TARGET
            ),
            1
        );

        // Same rejection, but the node then followed a compliant alternative up to
        // 966,010. Still ours — keying on "forks at the tip" refused this.
        assert_eq!(
            candidates(
                &[branch(966_010, 0, "active"), rejected.clone()],
                966_010,
                FLOOR,
                TARGET
            ),
            1
        );

        // Refused the first block above the floor, then activated a compliant
        // alternative that forks at the floor too and climbed well past it. The
        // invalid branch still forks *at* the floor, so requiring the fork to be
        // strictly above it refused this outcome as well — for the whole six hours,
        // keeping the rewind record alive and replaying the recovery on every restart.
        // What proves the node acted is its tip having climbed, not the fork point.
        let refused_at_floor = branch(TARGET, TARGET - FLOOR, "invalid");
        assert_eq!(
            candidates(
                &[branch(965_000, 0, "active"), refused_at_floor.clone()],
                965_000,
                FLOOR,
                TARGET
            ),
            1
        );

        // The same branch geometry while the tip is still *at* the floor is exactly
        // what our own `invalidateblock` leaves behind, and must never pass: reading it
        // as "re-checked and refused" would retire the recovery record with the chain
        // still parked below the invalidated block.
        assert_eq!(
            candidates(&[refused_at_floor], FLOOR, FLOOR, TARGET),
            0,
            "a parked chain must not look finished"
        );

        // An invalid branch that never reaches the height the chain started from is
        // not the one we replayed.
        assert_eq!(
            candidates(&[branch(963_000, 100, "invalid")], 965_999, FLOOR, TARGET),
            0
        );

        // Nor is one forking below the floor we rewound to.
        assert_eq!(
            candidates(
                &[branch(TARGET, TARGET - (FLOOR - 1), "invalid")],
                965_999,
                FLOOR,
                TARGET
            ),
            0
        );

        // A branch the node merely has headers for proves nothing: it never validated
        // it, so it cannot be why the chain stopped.
        assert_eq!(
            candidates(
                &[branch(TARGET, TARGET - 965_999, "headers-only")],
                965_999,
                FLOOR,
                TARGET
            ),
            0
        );

        // And nothing is a candidate while the tip is still below the floor — the
        // disconnect has not even been undone yet.
        assert_eq!(candidates(&[rejected], FLOOR - 10, FLOOR, TARGET), 0);
    }

    fn tip(height: i32, status: &str) -> coincubed::ChainTipEntry {
        use coincube_core::miniscript::bitcoin::{self, hashes::Hash};
        coincubed::ChainTipEntry {
            height,
            hash: bitcoin::BlockHash::from_byte_array([height as u8; 32]),
            branch_len: 0,
            status: status.to_string(),
        }
    }

    #[test]
    fn best_known_height_spans_every_branch() {
        assert_eq!(
            best_known_height(&[tip(100, "active"), tip(98, "valid-fork")]),
            Some(100)
        );
        assert_eq!(best_known_height(&[]), None);

        // Note the branch is `headers-only`, not `invalid`: a node that rejects a
        // block stops downloading that chain, so counting only `invalid` branches
        // would miss exactly the stranded case this exists for.
        assert_eq!(
            best_known_height(&[tip(100, "active"), tip(120, "headers-only")]),
            Some(120)
        );
        assert_eq!(
            best_known_height(&[tip(100, "active"), tip(120, "invalid")]),
            Some(120)
        );
    }

    #[test]
    fn the_ledger_round_trips_and_fails_safe() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-revalidate-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let datadir = CoincubeDirectory::new(dir.clone());

        // Absent sidecar reads as "never observed", not as an error.
        assert_eq!(ManagedNodeState::load(&datadir).last_run_flavor, None);

        ManagedNodeState::record_run(&datadir, NodeFlavor::Knots);
        assert_eq!(
            ManagedNodeState::load(&datadir).last_run_flavor,
            Some(NodeFlavor::Knots),
        );

        // Overwriting must replace, not append or corrupt.
        ManagedNodeState::record_run(&datadir, NodeFlavor::Core);
        assert_eq!(
            ManagedNodeState::load(&datadir).last_run_flavor,
            Some(NodeFlavor::Core),
        );

        // A corrupt sidecar fails safe to "unknown" for the flavour ledger...
        std::fs::write(ManagedNodeState::path(&datadir), b"{ not json").unwrap();
        assert_eq!(ManagedNodeState::load(&datadir).last_run_flavor, None);

        // ...but `try_load` must report it, because a caller acting on `rewind`
        // cannot be allowed to read "unreadable" as "nothing pending" and leave a
        // node parked below the block it invalidated.
        assert!(ManagedNodeState::try_load(&datadir).is_err());

        // A write must not clobber state it could not read: better to skip the
        // update than to erase a pending rewind.
        ManagedNodeState::record_run(&datadir, NodeFlavor::Core);
        assert!(ManagedNodeState::try_load(&datadir).is_err());
        assert!(!ManagedNodeState::set_rewind(&datadir, None));

        // A *missing* sidecar is different: that genuinely means "nothing yet".
        std::fs::remove_file(ManagedNodeState::path(&datadir)).unwrap();
        assert_eq!(
            ManagedNodeState::try_load(&datadir).unwrap(),
            ManagedNodeState::default()
        );

        // No stray temp file is left behind by the atomic write.
        assert!(!ManagedNodeState::path(&datadir)
            .with_extension("tmp")
            .exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
