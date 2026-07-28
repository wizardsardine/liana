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
    /// Whether the RDTS deployment is locked in or active on this node.
    pub deployment_live: bool,
    /// Oldest block the node still stores; `None` when it isn't pruned. Nothing at
    /// or below this can be disconnected — the data needed to do so is gone.
    pub prune_height: Option<i32>,
    /// Whether the node knows of a branch with more work than the one it follows
    /// (see [`needs_reconsider`]).
    ///
    /// This is what keeps the ledger from being load-bearing: a swap we failed to
    /// record still shows up here as an observable symptom.
    pub node_stranded: bool,
}

/// Why no remediation is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// RDTS isn't deployed on this network (regtest, signet, …).
    NoDeploymentOnNetwork,
    /// The deployment has not locked in, so no rule is being enforced and the two
    /// flavours cannot yet disagree.
    DeploymentNotLive,
    /// The chain has not reached the first height at which the flavours can
    /// diverge. Until mainnet passes 961,632 this is the universal answer.
    TipBelowAnchor,
    /// The node follows the best chain it knows of and did not just come from
    /// Knots. Nothing to clear.
    NothingToClear,
    /// Core → Knots, but pruning has already discarded everything above the
    /// anchor, so there is nothing left we are able to re-check.
    NothingRetainedAboveAnchor,
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
    if !facts.deployment_live {
        return RevalidationPlan::Skip(SkipReason::DeploymentNotLive);
    }
    // Nothing above the anchor exists yet, so nothing can have diverged.
    if facts.blocks <= anchor_height {
        return RevalidationPlan::Skip(SkipReason::TipBelowAnchor);
    }

    match facts.current_flavor {
        NodeFlavor::Core => {
            // Two independent triggers. The ledger notices the swap immediately;
            // the stranded check catches one we failed to record — a swap made
            // through the installer, a lost or corrupt sidecar, a datadir moved
            // between machines.
            let came_from_knots = facts.previous_flavor == Some(NodeFlavor::Knots);
            if came_from_knots || facts.node_stranded {
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
            let floor_height = match facts.prune_height {
                Some(prune_height) => anchor_height.max(prune_height + PRUNE_SAFETY_MARGIN),
                None => anchor_height,
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

    /// Load the ledger, or the empty default when the sidecar is absent or
    /// unreadable. Fail-safe: an unreadable ledger reads as "we've never seen this
    /// node run", which makes the next start record the truth rather than act on a
    /// guess.
    pub fn load(coincube_datadir: &CoincubeDirectory) -> Self {
        let path = Self::path(coincube_datadir);
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
                warn!("unreadable managed-node state at {path:?} ({e}); treating as unknown");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
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
    pub fn record_run(coincube_datadir: &CoincubeDirectory, flavor: NodeFlavor) {
        let mut state = Self::load(coincube_datadir);
        state.last_run_flavor = Some(flavor);
        if let Err(e) = state.save(coincube_datadir) {
            warn!("could not record managed-node flavour: {e}");
        }
    }

    /// Record (or clear) an in-flight rewind, preserving the flavour ledger.
    ///
    /// Returns whether the write succeeded: a rewind whose intent we could not
    /// persist must not be started, because we would have no way to finish it.
    pub fn set_rewind(
        coincube_datadir: &CoincubeDirectory,
        rewind: Option<RewindInFlight>,
    ) -> bool {
        let mut state = Self::load(coincube_datadir);
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
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// The stateless check
// ---------------------------------------------------------------------------

/// Whether the node is stranded off the best chain it knows about, and so needs a
/// `reconsiderblock`.
///
/// This is the backstop that makes the ledger non-load-bearing: it asks only "is
/// our active tip the most-work chain the node has headers for?", which is true of
/// a healthy node regardless of history.
///
/// Deliberately **not** keyed on a branch's status being `"invalid"`. A node that
/// rejects a block stops downloading that branch, so the honest most-work chain
/// commonly shows up as `headers-only` rather than `invalid` — gating on
/// `"invalid"` would miss exactly the stranded case this exists to catch.
pub fn needs_reconsider(tips: &[coincubed::ChainTipEntry]) -> bool {
    let Some(active_height) = tips.iter().find(|t| t.is_active()).map(|t| t.height) else {
        return false;
    };
    // Any other branch reaching higher than our active tip means the node knows of
    // more work than it is following.
    tips.iter()
        .any(|t| !t.is_active() && t.height > active_height)
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
pub fn reconcile_after_start(
    coincube_datadir: &CoincubeDirectory,
    bitcoind: &coincubed::BitcoinD,
    network: Network,
    observed_flavor: NodeFlavor,
) {
    // Before anything else: if a previous run died mid-rewind, the node is parked
    // below an invalidated block and will not move until we release it.
    resume_pending_rewind(coincube_datadir, bitcoind);

    // Record what we saw first. If the remediation below fails we would otherwise
    // never record it, and retry the same failing path on every single startup;
    // the stranded check is what catches the case regardless.
    let previous_flavor = ManagedNodeState::load(coincube_datadir).last_run_flavor;
    ManagedNodeState::record_run(coincube_datadir, observed_flavor);

    // Networks without the deployment cost us nothing: no RPCs at all.
    if rdts_anchor_height(network).is_none() {
        return;
    }

    let status = match bitcoind.chain_status() {
        Ok(status) => status,
        Err(e) => {
            warn!("could not read chain status for the RDTS flavour check: {e}");
            return;
        }
    };
    let deployment_live = bitcoind
        .deployment_status(RDTS_DEPLOYMENT)
        .map(|d| d.is_live())
        .unwrap_or(false);
    let node_stranded = match bitcoind.chain_tips() {
        Ok(tips) => needs_reconsider(&tips),
        Err(e) => {
            warn!("could not read chain tips for the RDTS flavour check: {e}");
            false
        }
    };

    let plan = plan(ChainFacts {
        network,
        previous_flavor,
        current_flavor: observed_flavor,
        blocks: status.blocks,
        deployment_live,
        prune_height: status.prune_height,
        node_stranded,
    });
    match plan {
        RevalidationPlan::Skip(reason) => {
            tracing::debug!("No RDTS revalidation needed ({reason:?}).");
        }
        RevalidationPlan::ClearFailureFlags { .. } => {
            if let Err(e) = execute(bitcoind, plan) {
                warn!("RDTS revalidation failed: {e}");
            }
        }
        RevalidationPlan::ReplayUnderRdts {
            floor_height,
            target_height,
        } => {
            if let Err(e) = execute_replay(coincube_datadir, bitcoind, floor_height, target_height)
            {
                warn!("RDTS replay failed: {e}");
            }
        }
    }
}

/// Finish a rewind that a previous run started but did not complete.
///
/// Without this, dying between `invalidateblock` and `reconsiderblock` leaves the
/// node parked below the invalidated block indefinitely: the failure flag is
/// persistent and nothing in the node records that we were mid-operation.
/// Idempotent, so running it when there is nothing to finish costs one RPC.
pub fn resume_pending_rewind(coincube_datadir: &CoincubeDirectory, bitcoind: &coincubed::BitcoinD) {
    let Some(rewind) = ManagedNodeState::load(coincube_datadir).rewind else {
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
    if let Some(prune_height) = status.prune_height {
        if floor_height <= prune_height + PRUNE_SAFETY_MARGIN {
            return Err(format!(
                "refusing to rewind to {floor_height}: pruning has reached {prune_height} and                  disconnecting pruned blocks would abort the node"
            ));
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

    let _maintenance = coincubed::MaintenanceGuard::new();
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
            deployment_live: true,
            // Unpruned unless a test says otherwise, so the prune floor isn't a
            // hidden variable in the cases that aren't about it.
            prune_height: None,
            node_stranded: false,
        }
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
            let mut f = facts(network, NodeFlavor::Knots, NodeFlavor::Core);
            // Even a stranded node: we have no defensible height to act from.
            f.node_stranded = true;
            assert_eq!(
                plan(f),
                RevalidationPlan::Skip(SkipReason::NoDeploymentOnNetwork),
            );
        }
    }

    // The runtime gate. Until RDTS locks in, both flavours enforce the same rules,
    // so no swap can strand the node and this must stay entirely inert.
    #[test]
    fn a_dormant_deployment_is_skipped() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core);
        f.deployment_live = false;
        f.node_stranded = true;
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::DeploymentNotLive)
        );
    }

    // As of writing mainnet has not reached 961,632, so this is what every real
    // node answers today. Shipping before activation is what lets the ledger be
    // seeded correctly on every install before divergence is possible at all.
    #[test]
    fn a_tip_below_the_anchor_is_skipped() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Core);
        f.blocks = RDTS_ANCHOR_MAINNET;
        assert_eq!(plan(f), RevalidationPlan::Skip(SkipReason::TipBelowAnchor));

        // One block into the mandatory-signalling window, divergence is possible.
        f.blocks = RDTS_ANCHOR_MAINNET + 1;
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
        let mut f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Core);
        f.previous_flavor = None;
        f.node_stranded = true;
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
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Knots);
        f.node_stranded = true;
        assert_eq!(plan(f), RevalidationPlan::Skip(SkipReason::NothingToClear));

        // Not even right after a Core -> Knots swap. That swap does trigger work,
        // but it must be a replay under RDTS — never a "repair" that clears the
        // rejections and drags the node back onto the chain the user just opted out
        // of following.
        let mut f = facts(Network::Bitcoin, NodeFlavor::Core, NodeFlavor::Knots);
        f.node_stranded = true;
        assert!(matches!(plan(f), RevalidationPlan::ReplayUnderRdts { .. }));
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
        f.prune_height = Some(prune_height);
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
        f.prune_height = Some(RDTS_ANCHOR_MAINNET - 50_000);
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
        f.prune_height = Some(f.blocks);
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::NothingRetainedAboveAnchor),
        );

        // And the margin counts: a floor that lands exactly on the tip is refused,
        // not attempted with a zero-length replay.
        f.prune_height = Some(f.blocks - PRUNE_SAFETY_MARGIN);
        assert_eq!(
            plan(f),
            RevalidationPlan::Skip(SkipReason::NothingRetainedAboveAnchor),
        );
    }

    // A rewind is only ever justified by an actual swap from Core. Restarting a
    // Knots node must not re-run it every time.
    #[test]
    fn a_restarting_knots_node_does_not_replay() {
        let mut f = facts(Network::Bitcoin, NodeFlavor::Knots, NodeFlavor::Knots);
        f.node_stranded = true;
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
    fn a_healthy_node_needs_no_reconsider() {
        assert!(!needs_reconsider(&[
            tip(100, "active"),
            tip(98, "valid-fork")
        ]));
        // No active tip at all (shouldn't happen) must not read as stranded.
        assert!(!needs_reconsider(&[]));
    }

    // Note the branch is `headers-only`, not `invalid`: a node that rejects a block
    // stops downloading that chain, so gating on `"invalid"` would miss exactly the
    // case this check exists for.
    #[test]
    fn a_stranded_node_needs_a_reconsider() {
        assert!(needs_reconsider(&[
            tip(100, "active"),
            tip(120, "headers-only")
        ]));
        assert!(needs_reconsider(&[tip(100, "active"), tip(120, "invalid")]));
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

        // A corrupt sidecar must fail safe to "unknown" rather than panic, so the
        // next start records the truth instead of acting on garbage.
        std::fs::write(ManagedNodeState::path(&datadir), b"{ not json").unwrap();
        assert_eq!(ManagedNodeState::load(&datadir).last_run_flavor, None);

        // No stray temp file is left behind by the atomic write.
        assert!(!ManagedNodeState::path(&datadir)
            .with_extension("tmp")
            .exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
