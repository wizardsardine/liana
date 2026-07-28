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
    thread,
    time::{Duration, Instant},
};

use coincube_core::miniscript::bitcoin::Network;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    dir::CoincubeDirectory,
    node::bitcoind::{internal_bitcoind_directory, NodeFlavor},
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
}

/// Write `contents` to `path` through a sibling temp file, flushing before the
/// rename so the visible file is either the old one or the complete new one.
///
/// The directory is fsynced after the rename as well. Without that, the file's
/// contents are durable but the directory entry pointing at them may not be, so a
/// crash can lose the rename and with it a rewind record — precisely the crash this
/// record exists to survive.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;

    // Unix only: Windows has no equivalent (a directory can't be opened as a file
    // without backup semantics), and NTFS metadata journalling makes the rename
    // durable there anyway.
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
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
    network: Network,
    observed_flavor: NodeFlavor,
) {
    // Before anything else: if a previous run died mid-rewind, the node is parked
    // below an invalidated block and will not move until we release it.
    resume_pending_rewind(coincube_datadir, bitcoind);

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
            if let Err(e) = execute(bitcoind, plan) {
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
    observed_flavor: NodeFlavor,
    floor_height: i32,
    target_height: i32,
) {
    // Claim maintenance here, not inside the thread, and hand the guard over. Several
    // Vaults can attach to the one shared node in the same instant; a check followed
    // by a set would let two of them each start rewinding it. The guard's Drop
    // releases on every exit path, including a spawn that never happens.
    let Some(guard) = coincubed::MaintenanceGuard::try_acquire() else {
        info!("A chain replay is already running; not starting another.");
        return;
    };
    let spawned = thread::Builder::new()
        .name("rdts-replay".to_string())
        .spawn(move || {
            // Held for the whole replay; released when this closure returns.
            let _guard = guard;
            let bitcoind = match coincubed::BitcoinD::new(&config, "rdts_replay".to_string()) {
                Ok(bitcoind) => bitcoind,
                Err(e) => {
                    warn!("could not connect to the managed node to replay the chain: {e}");
                    return;
                }
            };
            match execute_replay(&coincube_datadir, &bitcoind, floor_height, target_height) {
                // Only now has the swap actually been dealt with, so only now may the
                // ledger advance. Leaving it behind on failure is what makes the next
                // start retry, instead of seeing Knots → Knots and skipping forever —
                // this direction has no stateless backstop, because a Knots node
                // legitimately trailing the majority chain must never be "repaired".
                Ok(()) => ManagedNodeState::record_run(&coincube_datadir, observed_flavor),
                Err(e) => warn!("RDTS replay failed: {e}"),
            }
        });
    if let Err(e) = spawned {
        warn!("could not spawn the chain-replay thread: {e}");
    }
}

/// Finish a rewind that a previous run started but did not complete.
///
/// Without this, dying between `invalidateblock` and `reconsiderblock` leaves the
/// node parked below the invalidated block indefinitely: the failure flag is
/// persistent and nothing in the node records that we were mid-operation.
/// Idempotent, so running it when there is nothing to finish costs one RPC.
pub fn resume_pending_rewind(coincube_datadir: &CoincubeDirectory, bitcoind: &coincubed::BitcoinD) {
    // A replay running right now has a rewind recorded and a half-disconnected chain.
    // Another Vault attaching at that moment must not mistake it for a crash: issuing
    // `reconsiderblock` would cut the live replay off mid-disconnect and clear the
    // record out from under it, leaving the chain half-rewound with nothing to
    // finish it.
    if coincubed::managed_node_maintenance() {
        return;
    }
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
            return;
        }
    };
    let Some(rewind) = state.rewind else {
        return;
    };
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
        return;
    };
    match bitcoind.reconsider_block_noreply(&anchor) {
        Ok(()) => {
            ManagedNodeState::set_rewind(coincube_datadir, None);
        }
        Err(e) => warn!("could not finish the pending rewind: {e}"),
    }
}

/// How long to let each phase of a replay run before we stop watching it.
///
/// Expiry is not an error: the node keeps working through the reconnect on its own,
/// and the in-flight record means a later start can still finish the job. We simply
/// stop holding every Vault's poller while we wait.
const DISCONNECT_DEADLINE: Duration = Duration::from_secs(30 * 60);
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

    bitcoind
        .invalidate_block_noreply(&invalidate_hash)
        .map_err(|e| format!("invalidateblock at height {first_divergent} failed: {e}"))?;
    wait_for_tip(bitcoind, DISCONNECT_DEADLINE, |blocks| {
        blocks <= floor_height
    })?;

    info!("Rewind complete; reconnecting under BIP-110 rules.");
    bitcoind
        .reconsider_block_noreply(&floor_hash)
        .map_err(|e| format!("reconsiderblock at height {floor_height} failed: {e}"))?;
    let reached = wait_for_tip(bitcoind, RECONNECT_DEADLINE, |blocks| {
        blocks >= target_height
    })?;

    // Only now is the node whole again, so only now may the record go.
    ManagedNodeState::set_rewind(coincube_datadir, None);
    if reached < target_height {
        // Knots rejected something Core had accepted, or the deadline expired. Either
        // way the node is consistent; it just isn't where it started.
        warn!(
            "After re-checking, the node settled at height {reached} rather than {target_height}. \
             If this is not simply a slow reconnect, blocks accepted under Core were rejected \
             under BIP-110."
        );
    }
    Ok(())
}

/// Poll the node's height until `done`, the deadline expires, or the node stops
/// answering.
///
/// A node that stops answering after `invalidateblock` is the fatal-abort case —
/// disconnecting pruned data kills bitcoind rather than returning an error — so a
/// run of consecutive failures is treated as a hard error, never as success.
fn wait_for_tip(
    bitcoind: &coincubed::BitcoinD,
    deadline: Duration,
    done: impl Fn(i32) -> bool,
) -> Result<i32, String> {
    const MAX_CONSECUTIVE_FAILURES: u32 = 15;

    let started = Instant::now();
    let mut last_seen = None;
    let mut failures = 0u32;
    while started.elapsed() < deadline {
        match bitcoind.chain_status() {
            Ok(status) => {
                failures = 0;
                last_seen = Some(status.blocks);
                if done(status.blocks) {
                    return Ok(status.blocks);
                }
            }
            Err(e) => {
                failures += 1;
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
    last_seen.ok_or_else(|| "the managed node never reported a height".to_string())
}

/// Issue the `reconsiderblock` for `plan`, if it calls for one.
///
/// Fire-and-forget: re-activating the chain can reconnect many blocks and far
/// exceed the RPC socket timeout. Progress is observed out of band, from the
/// node's `getblockchaininfo` and its debug log.
pub fn execute(
    bitcoind: &coincubed::BitcoinD,
    plan: RevalidationPlan,
) -> Result<Option<i32>, String> {
    let RevalidationPlan::ClearFailureFlags { anchor_height } = plan else {
        return Ok(None);
    };
    let anchor = bitcoind
        .get_block_hash(anchor_height)
        .ok_or_else(|| format!("no block at the RDTS anchor height {anchor_height}"))?;
    info!(
        "Clearing inherited block-index rejections from the RDTS anchor \
         (height {anchor_height}, {anchor}) so this node can follow the most-work chain."
    );
    bitcoind
        .reconsider_block_noreply(&anchor)
        .map_err(|e| format!("reconsiderblock at height {anchor_height} failed: {e}"))?;
    Ok(Some(anchor_height))
}

#[cfg(test)]
mod tests {
    use super::*;

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
