use crate::{
    bitcoin::{
        maturity, AncestorSearch, BitcoinInterface, BlockChainTip, HistorySyncCache, PollFailure,
        ReorgAlertCache, UTxO, UTxOAddress,
    },
    database::{Coin, DatabaseConnection, DatabaseInterface},
};

use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync, thread, time,
};

#[cfg(test)]
use coincube_core::descriptors;
use miniscript::bitcoin::{self, secp256k1};

/// Deepest block chain reorganisation we will apply to our state.
///
/// Beyond this the far likelier explanation is a misreporting backend — a node
/// rewound with `invalidateblock`, a datadir swapped underneath us, a chainstate
/// mid-rebuild — than that Bitcoin genuinely undid this much history. Acting on it
/// is destructive and not fully reversible: `rollback_tip` clears the confirmation
/// state of every coin above the ancestor (which alone disables coin selection and
/// the recovery sweep, both of which require `block_info`), and the following
/// `update_coins` pass hard-deletes any deposit that is neither confirmed on the
/// new chain nor still in the local mempool. So we decline and keep our state,
/// rather than corrupting it on a backend we have reason to distrust.
///
/// 500 blocks is ~3.5 days of chain. The deepest reorg ever observed on mainnet is
/// 53 blocks (August 2010, the value overflow incident: the corrected chain
/// overtook the bad one at height 74691, 53 above the offending block 74638). The
/// deepest since is 24 blocks (March 2013, the v0.7/v0.8 split). So this is a
/// generous margin over both.
pub const MAX_REORG_DEPTH: i32 = 500;

/// How many times to retry a failed common-ancestor lookup within a single poll
/// before giving up and leaving our state untouched until the next one.
const ANCESTOR_LOOKUP_ATTEMPTS: u32 = 3;

fn ancestor_lookup_retry_interval() -> time::Duration {
    #[cfg(not(test))]
    {
        time::Duration::from_secs(2)
    }
    #[cfg(test)]
    time::Duration::ZERO
}

#[derive(Debug, Clone)]
struct UpdatedCoins {
    pub receive_index: bitcoin::bip32::ChildNumber,
    pub change_index: bitcoin::bip32::ChildNumber,
    pub received: Vec<Coin>,
    pub confirmed: Vec<(bitcoin::OutPoint, i32, u32)>,
    pub expired: Vec<bitcoin::OutPoint>,
    pub spending: Vec<(bitcoin::OutPoint, bitcoin::Txid)>,
    pub expired_spending: Vec<bitcoin::OutPoint>,
    pub spent: Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)>,
}

#[derive(Clone, Copy)]
struct ReceiveIndices {
    receive_index: bitcoin::bip32::ChildNumber,
    change_index: bitcoin::bip32::ChildNumber,
    initial_highest: u32,
}

enum ReceivedDerivation {
    SkipInvalidNetwork,
    Unresolved,
    Mapped(bitcoin::bip32::ChildNumber, bool),
}

// Update the state of our coins. There may be new unspent, and existing ones may become confirmed
// or spent.
// NOTE: A coin may be updated multiple times at once. That is, a coin may be received, confirmed,
// and spent in a single poll.
// NOTE: Coinbase transaction deposits are treated as unconfirmed until the CB tx matures. Backends
// only know Bitcoin's 100 blocks; `hold_immature_coinbase` then applies the chain's own rule.
fn update_coins(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    previous_tip: &BlockChainTip,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    _secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<UpdatedCoins, String> {
    let network = db_conn.network();
    let curr_coins = db_conn.coins(&[], &[]);
    let receive_index = db_conn.receive_index();
    let change_index = db_conn.change_index();
    let initial_highest = u32::from(receive_index.max(change_index));
    log::debug!("Current coins: {:?}", curr_coins);
    let mut indices = ReceiveIndices {
        receive_index,
        change_index,
        initial_highest,
    };

    let received = discover_received_coins(
        bit,
        db_conn,
        previous_tip,
        descs,
        network,
        &curr_coins,
        &mut indices,
    )?;

    // We need to take the newly received ones into account as well, as they may have been
    // confirmed within the previous tip and the current one, and we may not poll this chunk of the
    // chain anymore.
    let to_be_confirmed: Vec<bitcoin::OutPoint> = curr_coins
        .values()
        .chain(received.iter())
        .filter_map(|coin| {
            if coin.block_info.is_none() {
                Some(coin.outpoint)
            } else {
                None
            }
        })
        .collect();
    let (confirmed, expired) = bit.try_confirmed_coins(&to_be_confirmed)?;
    log::debug!("Newly confirmed coins: {:?}", confirmed);
    log::debug!("Expired coins: {:?}", expired);

    // We need to take the newly received ones into account as well, as they may have been
    // spent within the previous tip and the current one, and we may not poll this chunk of the
    // chain anymore.
    // NOTE: curr_coins contain the "spending" coins. So this takes care of updating the spend_txid
    // if a coin's spending transaction gets RBF'd.
    let expired_set: HashSet<_> = expired.iter().collect();
    let to_be_spent: Vec<bitcoin::OutPoint> = curr_coins
        .values()
        .chain(received.iter())
        .filter_map(|coin| {
            // Always check for spends when the spend tx is not confirmed as it might get RBF'd.
            if (coin.spend_txid.is_some() && coin.spend_block.is_some())
                || expired_set.contains(&coin.outpoint)
            {
                None
            } else {
                Some(coin.outpoint)
            }
        })
        .collect();
    let spending = bit.try_spending_coins(&to_be_spent)?;
    log::debug!("Newly spending coins: {:?}", spending);

    // Mark coins in a spending state whose Spend transaction was confirmed as such. Note we
    // need to take into account the freshly marked as spending coins as well, as their spend
    // may have been confirmed within the previous tip and the current one, and we may not poll
    // this chunk of the chain anymore.
    let spending_coins: Vec<(bitcoin::OutPoint, bitcoin::Txid)> = db_conn
        .list_spending_coins()
        .values()
        .map(|coin| (coin.outpoint, coin.spend_txid.expect("Coin is spending")))
        .chain(spending.iter().cloned())
        .collect();
    let (spent, expired_spending) = bit.try_spent_coins(spending_coins.as_slice())?;
    log::debug!("Newly spent coins: {:?}", spent);

    Ok(UpdatedCoins {
        receive_index: indices.receive_index,
        change_index: indices.change_index,
        received,
        confirmed,
        expired,
        spending,
        expired_spending,
        spent,
    })
}

fn find_staged_address_index(
    bit: &impl BitcoinInterface,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    address: &bitcoin::Address,
    network: bitcoin::Network,
    indices: &ReceiveIndices,
) -> Result<Option<(bitcoin::bip32::ChildNumber, bool)>, String> {
    let highest = u32::from(indices.receive_index.max(indices.change_index));
    let gap = crate::database::sqlite::LOOK_AHEAD_LIMIT;
    let start = indices
        .initial_highest
        .checked_add(gap)
        .ok_or("Derivation range overflow")?;
    let mut end = highest
        .checked_add(gap)
        .ok_or("Derivation range overflow")?;
    if bit.wallet_record_replay_pending().is_some() {
        end = end.max(1001);
    }
    let mut found = None;
    for index in start..end {
        let child = bitcoin::bip32::ChildNumber::from_normal_idx(index)
            .map_err(|_| "Derivation range exhausted")?;
        for (branch, desc) in descs.iter().enumerate().take(2) {
            if desc
                .at_derivation_index(u32::from(child))
                .map_err(|error| error.to_string())?
                .address(network)
                .map_err(|error| error.to_string())?
                == *address
            {
                found = Some((child, branch == 1));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    Ok(found)
}

fn resolve_received_derivation(
    address: &UTxOAddress,
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    network: bitcoin::Network,
    indices: &ReceiveIndices,
) -> Result<ReceivedDerivation, String> {
    Ok(match address {
        UTxOAddress::Address(address) => {
            let address = match address.clone().require_network(network) {
                Ok(addr) => addr,
                Err(e) => {
                    log::error!("Invalid network for address: {}", e);
                    return Ok(ReceivedDerivation::SkipInvalidNetwork);
                }
            };
            if let Some((derivation_index, is_change)) =
                db_conn.derivation_index_by_address(&address)
            {
                ReceivedDerivation::Mapped(derivation_index, is_change)
            } else {
                // Earlier deposits in this batch may extend the lookahead.
                // Resolve that extension in memory, keeping DB writes deferred.
                let found = find_staged_address_index(bit, descs, &address, network, indices)?;
                match found {
                    Some((index, is_change)) => ReceivedDerivation::Mapped(index, is_change),
                    None => ReceivedDerivation::Unresolved,
                }
            }
        }
        UTxOAddress::DerivIndex(index, is_change) => ReceivedDerivation::Mapped(*index, *is_change),
    })
}

fn advance_received_indices(
    indices: &mut ReceiveIndices,
    derivation_index: bitcoin::bip32::ChildNumber,
    is_change: bool,
) {
    // First of if we are receiving coins that are beyond our next derivation index,
    // adjust it.
    // Stage index changes with the coin updates. Later backend reads may
    // fail or reveal a changed tip, in which case nothing from this batch
    // should have been written yet.
    if !is_change && derivation_index > indices.receive_index {
        indices.receive_index = derivation_index;
    } else if is_change && derivation_index > indices.change_index {
        indices.change_index = derivation_index;
    }
}

fn next_received_batch(
    bit: &impl BitcoinInterface,
    unresolved: Vec<UTxO>,
    before: usize,
) -> Result<Option<Vec<UTxO>>, String> {
    // A later entry may have widened the staged range. Revisit only
    // unresolved entries until no more can be mapped, independent of RPC
    // ordering. Unknown outliers must not stall the entire wallet poll.
    if unresolved.is_empty() {
        return Ok(None);
    }
    if unresolved.len() == before {
        if bit.wallet_record_replay_requires_complete_mapping() {
            return Err("Recovered wallet outputs could not be mapped within the prepared address range; restore the Vault's address indices before completing recovery".into());
        }
        for utxo in unresolved {
            log::error!(
                "Cannot map owned coin {} to a derivation index; skipping it for now",
                utxo.outpoint
            );
        }
        return Ok(None);
    }
    Ok(Some(unresolved))
}

fn discover_received_coins(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    previous_tip: &BlockChainTip,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    network: bitcoin::Network,
    curr_coins: &std::collections::HashMap<bitcoin::OutPoint, Coin>,
    indices: &mut ReceiveIndices,
) -> Result<Vec<Coin>, String> {
    // Start by fetching newly received coins.
    let mut received = Vec::new();
    // An imported wallet may expand its discovery window after the database
    // tip already passed the funding blocks. During that pending historical
    // scan, offer all backend-discovered outputs and let the durable outpoint
    // set below de-duplicate them. A tip-bounded query would permanently hide
    // those old outputs and falsely mark the larger scan complete at zero.
    let mut pending = if db_conn
        .imported_history()
        .is_some_and(|history| !history.history_complete)
    {
        bit.try_historical_received_coins(previous_tip, descs)?
    } else {
        bit.try_received_coins(previous_tip, descs)?
    };
    loop {
        let before = pending.len();
        let mut unresolved = Vec::new();
        for utxo in pending {
            let UTxO {
                outpoint,
                amount,
                ref address,
                is_immature,
                ..
            } = utxo;
            // We can only really treat them if we know the derivation index that was used.
            let (derivation_index, is_change) = match resolve_received_derivation(
                address, bit, db_conn, descs, network, indices,
            )? {
                ReceivedDerivation::SkipInvalidNetwork => continue,
                ReceivedDerivation::Unresolved => {
                    unresolved.push(utxo);
                    continue;
                }
                ReceivedDerivation::Mapped(index, is_change) => (index, is_change),
            };
            advance_received_indices(indices, derivation_index, is_change);

            // Now record this coin as a newly received one.
            if !curr_coins.contains_key(&outpoint) {
                let coin = Coin {
                    outpoint,
                    is_immature,
                    amount,
                    derivation_index,
                    is_change,
                    block_info: None,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                };
                received.push(coin);
            }
        }
        match next_received_batch(bit, unresolved, before)? {
            Some(next) => pending = next,
            None => break,
        }
    }
    log::debug!("Newly received coins: {:?}", received);

    Ok(received)
}

// Add new deposit and spend transactions to the database.
fn collect_txs(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    updated_coins: &UpdatedCoins,
) -> Result<Vec<bitcoin::Transaction>, String> {
    let curr_txids: HashSet<_> = db_conn.list_saved_txids().into_iter().collect();
    let mut new_txids = HashSet::new();
    // Get the transaction for all newly received coins. Note we also query it if the coins
    // expired, as it's possible for coin to not be in DB already (and therefore not have its
    // deposit transaction stored there), to be marked as expired *and* newly received. In this
    new_txids.extend(updated_coins.received.iter().map(|c| c.outpoint.txid));

    // Add spend txid for new & existing coins.
    new_txids.extend(updated_coins.spending.iter().map(|(_, txid)| txid));

    // Remove those txids we already have.
    let missing_txids = new_txids.difference(&curr_txids);
    log::debug!("Missing txids: {:?}", missing_txids);

    // Now retrieve txs.
    let txs: Vec<_> = missing_txids
        .map(|txid| {
            bit.try_wallet_transaction(txid)?
                .map(|(tx, _)| tx)
                .ok_or_else(|| "Wallet transaction disappeared during polling".to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(txs)
}

#[derive(Debug, Clone, Copy)]
enum TipUpdate {
    // The best block is still the same as in the previous poll.
    Same,
    // There is a new best block that extends the same chain.
    Progress(BlockChainTip),
    // There is a new best block that extends a chain which does not contain our former tip.
    Reorged(BlockChainTip),
    // The backend reports a reorg deeper than `MAX_REORG_DEPTH`. We decline to apply it and
    // leave our state alone; see that constant for why. `min_depth` is a lower bound: once
    // the ancestor walk hits its limit we stop paying for round-trips, so past that point we
    // know only that the fork is at least this deep.
    ImplausibleReorg { min_depth: i32 },
    // Our tip is not on the backend's chain, the backend reported no reorg to roll back
    // to, and it is one of the backends that cannot be asked where the chains parted.
    // Skip the poll and leave our state alone; see the `walks_common_ancestor` check
    // below for how we get here and how we get out again.
    Diverged { backend_tip: BlockChainTip },
    // We could not establish how the backend's chain relates to ours. Skip this poll rather
    // than update state from a backend we could not read.
    Unavailable,
}

// Returns the new block chain tip, if it changed.
fn new_tip(bit: &impl BitcoinInterface, current_tip: &BlockChainTip) -> TipUpdate {
    // A failed common-ancestor lookup used to re-enter this function immediately and without
    // bound. On a backend that keeps failing that is an unthrottled RPC spin loop which also
    // grows the stack on every turn, since the call is not guaranteed to be optimised into a
    // jump. Retry a few times with a pause, then give up until the next poll.
    for attempt in 1..=ANCESTOR_LOOKUP_ATTEMPTS {
        if let Some(update) = observe_tip_once(bit, current_tip) {
            return update;
        }

        log::error!(
            "Failed to get common ancestor for tip '{}' (attempt {}/{}).",
            current_tip,
            attempt,
            ANCESTOR_LOOKUP_ATTEMPTS
        );
        if attempt < ANCESTOR_LOOKUP_ATTEMPTS {
            thread::sleep(ancestor_lookup_retry_interval());
        }
    }

    log::error!(
        "Giving up on the common ancestor lookup for tip '{}' after {} attempts. Leaving our \
         state untouched; will retry on the next poll.",
        current_tip,
        ANCESTOR_LOOKUP_ATTEMPTS
    );
    TipUpdate::Unavailable
}

fn observe_tip_once(bit: &impl BitcoinInterface, current_tip: &BlockChainTip) -> Option<TipUpdate> {
    let Ok(bitcoin_tip) = bit.try_chain_tip() else {
        return Some(TipUpdate::Unavailable);
    };

    // If the tip didn't change, there is nothing to update.
    if current_tip == &bitcoin_tip {
        return Some(TipUpdate::Same);
    }

    if bitcoin_tip.height > current_tip.height {
        // Make sure we are on the same chain.
        if match bit.try_is_in_chain(current_tip) {
            Ok(in_chain) => in_chain,
            Err(_) => return Some(TipUpdate::Unavailable),
        } {
            // All good, we just moved forward.
            return Some(TipUpdate::Progress(bitcoin_tip));
        }
    }

    // Either the new height is lower or the same but the block hash differs. There was a
    // block chain re-organisation. Find the common ancestor between our current chain and
    // the new chain and return that. The caller will take care of rewinding our state.

    // Unless this is a backend that has no ancestor to give. Electrum and Esplora report
    // a reorg the moment they see one, from `sync_wallet`, and keep nothing of our chain
    // to walk back through afterwards — so reaching here on one of them does not mean a
    // reorg just happened, it means our tip and theirs are still apart from an earlier
    // one we refused to apply. Asking anyway used to hit an `unreachable!()` and, with
    // `panic = "abort"`, take the whole app down on the very next poll after any refusal.
    if !bit.walks_common_ancestor() {
        return Some(TipUpdate::Diverged {
            backend_tip: bitcoin_tip,
        });
    }

    log::info!("Block chain reorganization detected. Looking for common ancestor.");
    // Bounded at one past the limit: any fork we would accept is found inside it, and
    // anything deeper is refused without walking the rest of the way. Unbounded, a
    // backend claiming a 10k-block reorg would cost us 10k sequential round-trips before
    // we rejected the answer.
    match bit.common_ancestor(current_tip, MAX_REORG_DEPTH + 1) {
        AncestorSearch::Found(common_ancestor) => {
            let depth = current_tip.height.saturating_sub(common_ancestor.height);
            if depth > MAX_REORG_DEPTH {
                return Some(TipUpdate::ImplausibleReorg { min_depth: depth });
            }
            log::info!(
                "Common ancestor found: '{}'. Starting rescan from there. Old tip was '{}'.",
                common_ancestor,
                current_tip
            );
            return Some(TipUpdate::Reorged(common_ancestor));
        }
        // An answer, not a failure: retrying would just spend the same round-trips again
        // to reach the same conclusion. Deeper than we were willing to walk.
        AncestorSearch::TooDeep => {
            return Some(TipUpdate::ImplausibleReorg {
                min_depth: MAX_REORG_DEPTH + 1,
            })
        }
        AncestorSearch::Failed => {}
    }

    None
}

/// Whether a refused reorg's depth was measured or merely bounded.
#[derive(Debug, Clone, Copy)]
enum RefusedDepth {
    /// The true depth: either we walked to the fork point, or (when the backend handed us
    /// the ancestor) it is how deep the refused rollback reaches; see [`rollback_depth`].
    Exact(i32),
    /// The ancestor walk stopped at its limit, so the fork is at least this deep.
    AtLeast(i32),
}

/// Decline a reorganization deeper than [`MAX_REORG_DEPTH`]: say so loudly, publish it
/// for `get_info`, and leave wallet state alone.
///
/// Shared by both paths that can encounter one — the ancestor walk in [`new_tip`], and
/// the ancestor an Electrum-style backend hands us directly from `sync_wallet`. The
/// refusal has to be identical either way; keeping two copies is how their messages
/// drifted apart once already.
fn refuse_deep_reorg(
    reorg_alert: &ReorgAlertCache,
    current_tip: &BlockChainTip,
    depth: RefusedDepth,
) {
    let (depth, bound) = match depth {
        RefusedDepth::Exact(depth) => (depth, ""),
        RefusedDepth::AtLeast(depth) => (depth, " or more"),
    };
    log::error!(
        "Backend reports a reorganization of {} blocks{} from our tip '{}', deeper than the \
         {}-block limit. Refusing to roll back our state: this is far more likely a misreporting \
         backend than real chain history being undone. Wallet state is left untouched and no \
         coins are removed.",
        depth,
        bound,
        current_tip,
        MAX_REORG_DEPTH
    );
    reorg_alert.store_refused_reorg(depth);
}

/// Report that our tip is off the backend's chain with no reorg outstanding to
/// explain it, and that this backend cannot tell us where they parted.
///
/// This is the state a refused reorg leaves behind: we deliberately kept our tip, so
/// it no longer matches theirs, and Electrum/Esplora only report a fork at the moment
/// they observe it. Nothing here can be resolved safely — rolling back would mean
/// guessing at a fork point, and moving forward would mean recording blocks from a
/// chain our own tip isn't on — so the poll is skipped and wallet state left exactly
/// as it is.
///
/// It resolves itself rather than needing a restart: the backend keeps syncing, and as
/// soon as its chain contains our tip again (a provider that was serving a truncated
/// or foreign chain recovers, or a rescan rebuilds ours) the next poll takes the
/// ordinary forward-progress path, which clears the alert.
fn report_divergence(
    reorg_alert: &ReorgAlertCache,
    current_tip: &BlockChainTip,
    backend_tip: &BlockChainTip,
) {
    log::warn!(
        "Our tip '{}' is not on the backend's chain, which is at '{}', and the backend reports \
         no reorganization to roll back to. This is what a refused reorganization leaves behind; \
         this backend cannot be asked where the two chains parted. Skipping this poll with wallet \
         state untouched. It will resume on its own once our tip is back on the backend's chain.",
        current_tip,
        backend_tip
    );
    // Publish it for `get_info` as an explicit divergence — a *distinct* status from a
    // refused deep reorg. We must not fold it into `refused_reorg_depth`: this backend
    // cannot tell us where the chains parted, so the true fork could be far deeper than
    // the tips' height gap, and when the backend sits at or above our height there is no
    // rollback at all. Recording a number here would misrepresent an unknown divergence
    // as an exact rollback depth, and a zero gap would read as "no alert" — silently
    // hiding a poll that is in fact paused, most visibly right after a daemon restart
    // when no earlier alert survives in memory to paper over it.
    reorg_alert.store_divergence();
}

/// How far below our tip rolling back to `ancestor` reaches into recorded history: the
/// depth of the deepest coin or spend confirmation it would undo, or zero if it undoes
/// none.
///
/// This, rather than the gap down to `ancestor`, is what the depth guard measures when the
/// backend hands us the ancestor (Electrum, Esplora). Their ancestor is the nearest block
/// their sparse local chain still shares with the backend, not the fork point, and after a
/// restart that chain holds only genesis and our tip. So the gap shows how little of the
/// chain we kept, not how much the backend undid. Measured that way, a stale tip on a
/// wallet left idle for a week read as a reorg of a thousand blocks, or of the whole chain
/// for a wallet with no confirmed coins. It was refused, and every later poll then saw a
/// divergence it could not resolve, so the wallet stopped syncing for good.
///
/// `rollback_tip` only clears confirmations above the ancestor, so this is exactly what the
/// rollback would destroy. A backend that drops our confirmed history deeper than
/// [`MAX_REORG_DEPTH`] is still refused.
fn rollback_depth(
    db_conn: &mut Box<dyn DatabaseConnection>,
    current_tip: &BlockChainTip,
    ancestor: &BlockChainTip,
) -> i32 {
    db_conn
        .coins(&[], &[])
        .values()
        .flat_map(|coin| [coin.block_info, coin.spend_block])
        .flatten()
        .map(|block| block.height)
        .filter(|height| *height > ancestor.height)
        .min()
        .map_or(0, |deepest| {
            current_tip
                .height
                .saturating_sub(deepest)
                .saturating_add(1)
                .max(0)
        })
}

/// Apply the chain's coinbase maturity on top of the backends' 100-block floor.
///
/// Backends confirm a coinbase deposit once it is 100 blocks deep, which is Bitcoin's rule
/// but not Bitcoin Blake2b's (see [`maturity`]). Before anything is written this:
///
/// - drops newly confirmed coinbase deposits that are not mature at `tip_height` from
///   `updated.confirmed`, so they stay unconfirmed and are re-checked on every poll;
/// - flags newly received coinbase deposits that are not mature as immature;
/// - returns the stored coins to put (back) into the immature state: coinbase deposits that
///   were confirmed before reaching the chain's maturity (by an earlier build, which used 100
///   blocks everywhere) and unconfirmed ones that lost their flag (a rollback leaves it unset).
///
/// A deposit that already has a spender is left alone: a node accepted that spend, so the
/// deposit was mature for it. Change coins cannot carry the immature flag (the schema forbids
/// it); they are only kept unconfirmed.
///
/// Whether a coin comes from a coinbase transaction is read from the transaction itself
/// (`coinbase_txids`), not from the backend's immature flag, which follows the 100-block rule.
fn hold_immature_coinbase(
    chain: coincube_core::chain::ChainId,
    tip_height: i32,
    coinbase_txids: &HashSet<bitcoin::Txid>,
    curr_coins: &HashMap<bitcoin::OutPoint, Coin>,
    updated: &mut UpdatedCoins,
) -> Vec<bitcoin::OutPoint> {
    let is_coinbase = |op: &bitcoin::OutPoint| coinbase_txids.contains(&op.txid);
    let spending: HashSet<bitcoin::OutPoint> = updated.spending.iter().map(|(op, _)| *op).collect();
    let expired: HashSet<bitcoin::OutPoint> = updated.expired.iter().copied().collect();
    let has_spender = |op: &bitcoin::OutPoint| {
        spending.contains(op) || curr_coins.get(op).is_some_and(|c| c.spend_txid.is_some())
    };
    // `None` for the RDTS expiry input: unknown is treated as covered, never shorter.
    let mature = |height: i32| maturity::coinbase_is_mature(chain, height, tip_height, None);

    updated.confirmed.retain(|(op, height, _)| {
        let keep = !is_coinbase(op) || has_spender(op) || mature(*height);
        if !keep {
            log::debug!(
                "Coinbase deposit at '{}' (block {}) is not mature on {} at tip {}. \
                 Not marking it as confirmed for now.",
                op,
                height,
                chain,
                tip_height
            );
        }
        keep
    });
    let confirmed: HashSet<bitcoin::OutPoint> =
        updated.confirmed.iter().map(|(op, ..)| *op).collect();

    for coin in updated.received.iter_mut() {
        let op = coin.outpoint;
        if !is_coinbase(&op) || confirmed.contains(&op) || expired.contains(&op) || has_spender(&op)
        {
            continue;
        }
        if coin.is_change {
            log::warn!(
                "Immature coinbase deposit at '{}' pays to a change address.",
                op
            );
        } else {
            coin.is_immature = true;
        }
    }

    curr_coins
        .values()
        .filter(|c| {
            let op = &c.outpoint;
            if !is_coinbase(op) || c.is_change || expired.contains(op) || has_spender(op) {
                return false;
            }
            match c.block_info {
                Some(block) => !mature(block.height),
                None => !c.is_immature && !confirmed.contains(op),
            }
        })
        .map(|c| {
            log::info!(
                "Coinbase deposit at '{}' is not mature on {} at tip {}. Marking it immature.",
                c.outpoint,
                chain,
                tip_height
            );
            c.outpoint
        })
        .collect()
}

fn updates(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut impl BitcoinInterface,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    reorg_alert: &ReorgAlertCache,
) -> Result<(), String> {
    // Check if there was a new block before we update our state.
    //
    // Some backends (such as Electrum) need to perform an explicit sync to provide updated data
    // about the Bitcoin network. For those the common ancestor is immediately returned in case
    // there was a reorg. For other backends (such as bitcoind) this function always return
    // `Ok(None)`. We leverage this to query the next tip and poll for reorgs only in this case.
    // FIXME: harmonize the Bitcoin backend interface, this intricacy is due to the introduction of
    // an Electrum backend with the bitcoind-specific backend interface.
    let current_tip = db_conn.chain_tip().expect("Always set at first startup");
    let (receive_index, change_index) = (db_conn.receive_index(), db_conn.change_index());
    let latest_tip = match bit.sync_wallet(receive_index, change_index) {
        Ok(None) => {
            match new_tip(bit, &current_tip) {
                TipUpdate::Same => current_tip,
                TipUpdate::Progress(new_tip) => new_tip,
                TipUpdate::Reorged(new_tip) => {
                    // The block chain was reorganized. Rollback our state down to the common ancestor
                    // between our former chain and the new one, then restart fresh.
                    db_conn.rollback_tip(&new_tip);
                    log::info!("Tip was rolled back to '{}'.", new_tip);
                    return Err("Chain rollback requires a new scheduled poll".into());
                }
                TipUpdate::ImplausibleReorg { min_depth } => {
                    refuse_deep_reorg(reorg_alert, &current_tip, RefusedDepth::AtLeast(min_depth));
                    return Err("Poll did not produce a coherent chain update".into());
                }
                TipUpdate::Diverged { backend_tip } => {
                    report_divergence(reorg_alert, &current_tip, &backend_tip);
                    return Err("Poll did not produce a coherent chain update".into());
                }
                TipUpdate::Unavailable => {
                    return Err("Backend unavailable during tip lookup".into())
                }
            }
        }
        Ok(Some(reorg_common_ancestor)) => {
            // The block chain was reorganized. Rollback our state down to the common ancestor
            // between our former chain and the new one, then restart fresh.
            // Make sure the common ancestor is not higher than the current DB tip, which could
            // happen if a rescan has been detected and the DB tip rolled back accordingly.
            if reorg_common_ancestor.height <= current_tip.height
                // check hash in case height is the same
                && reorg_common_ancestor.hash != current_tip.hash
            {
                // Same depth guard as the `new_tip` path above: backends that hand us the
                // common ancestor directly (Electrum, Esplora) can misreport just as badly as
                // those we walk back ourselves. But their ancestor is not the fork point, so
                // the guard measures what the rollback would undo; see `rollback_depth`.
                let depth = rollback_depth(db_conn, &current_tip, &reorg_common_ancestor);
                if depth > MAX_REORG_DEPTH {
                    refuse_deep_reorg(reorg_alert, &current_tip, RefusedDepth::Exact(depth));
                    return Err("Poll did not produce a coherent chain update".into());
                }
                db_conn.rollback_tip(&reorg_common_ancestor);
                log::info!("Tip was rolled back to '{}'.", reorg_common_ancestor);
            } else {
                log::info!(
                    "Tip was already earlier than common ancestor '{}'.",
                    reorg_common_ancestor
                );
            }
            return Err("Chain rollback requires a new scheduled poll".into());
        }
        Err(e) => return Err(e),
    };

    // Then check the state of our coins. Do it even if the tip did not change since last poll, as
    // we may have unconfirmed transactions.
    let mut updated_coins = update_coins(bit, db_conn, &current_tip, descs, secp)?;

    let transactions = collect_txs(bit, db_conn, &updated_coins)?;
    let coinbase_txids: HashSet<bitcoin::Txid> = db_conn
        .coinbase_txids()
        .into_iter()
        .chain(
            transactions
                .iter()
                .filter(|tx| tx.is_coinbase())
                .map(|tx| tx.compute_txid()),
        )
        .collect();
    let to_mark_immature = if coinbase_txids.is_empty() {
        Vec::new()
    } else {
        let chain = db_conn.chain();
        let curr_coins = db_conn.coins(&[], &[]);
        hold_immature_coinbase(
            chain,
            latest_tip.height,
            &coinbase_txids,
            &curr_coins,
            &mut updated_coins,
        )
    };
    // Check again after all transaction reads, before the first write.
    if bit.try_chain_tip()? != latest_tip {
        return Err("Chain tip changed during polling; retry on the next scheduled poll".into());
    }

    // Transactions must be added to the DB before coins due to foreign key constraints.
    if !transactions.is_empty() {
        db_conn.new_txs(&transactions);
    }
    reorg_alert.clear();
    if updated_coins.receive_index > db_conn.receive_index() {
        db_conn.set_receive_index(updated_coins.receive_index, secp);
    }
    if updated_coins.change_index > db_conn.change_index() {
        db_conn.set_change_index(updated_coins.change_index, secp);
    }
    // The chain tip did not change since we started our updates. Record them and the latest tip.
    // Having the tip in database means that, as far as the chain is concerned, we've got all
    // updates up to this block. But not more.
    db_conn.new_unspent_coins(&updated_coins.received);
    db_conn.remove_coins(&updated_coins.expired);
    db_conn.confirm_coins(&updated_coins.confirmed);
    if !to_mark_immature.is_empty() {
        db_conn.mark_immature_coinbase(&to_mark_immature);
    }
    db_conn.unspend_coins(&updated_coins.expired_spending);
    db_conn.spend_coins(&updated_coins.spending);
    db_conn.confirm_spend(&updated_coins.spent);
    // Update info about which coins are from self only after
    // coins have been inserted & updated above.
    db_conn.update_coins_from_self(current_tip.height);
    if latest_tip != current_tip {
        db_conn.update_tip(&latest_tip);
        log::debug!("New tip: '{}'", latest_tip);
    }

    log::debug!("Updates done.");
    Ok(())
}

// Read rescan state before any writes from this poll. An RPC failure is not
// evidence that a previously requested rescan has finished.
enum RescanObservation {
    Idle,
    Running(f64),
    Completed(BlockChainTip),
}

fn observe_rescan(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &impl BitcoinInterface,
) -> Result<RescanObservation, String> {
    if let Some(progress) = bit.try_rescan_progress()? {
        return Ok(RescanObservation::Running(progress));
    }
    if let Some(timestamp) = db_conn.rescan_timestamp() {
        return bit
            .try_block_before_date(timestamp)?
            .map(RescanObservation::Completed)
            .ok_or_else(|| "Could not retrieve the rescan starting block".into());
    }
    Ok(RescanObservation::Idle)
}

/// If the database chain tip is NULL (first startup), initialize it.
pub fn maybe_initialize_tip(
    bit: &impl BitcoinInterface,
    db: &impl DatabaseInterface,
) -> Result<(), crate::bitcoin::GenesisError> {
    let mut db_conn = db.connection();

    if db_conn.chain_tip().is_none() {
        // TODO: be smarter. We can use the timestamp of the descriptor to get a newer block hash.
        db_conn.update_tip(&bit.genesis_block()?);
    }
    Ok(())
}

pub fn sync_poll_interval() -> time::Duration {
    // TODO: be smarter, like in revaultd, but more generic too.
    #[cfg(not(test))]
    {
        time::Duration::from_secs(30)
    }
    #[cfg(test)]
    time::Duration::from_secs(0)
}

/// Update our state from the Bitcoin backend.
pub fn poll(
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    descs: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    reorg_alert: &ReorgAlertCache,
    history_sync: &HistorySyncCache,
) {
    let mut db_conn = db.connection();
    let replay_ticket = bit.wallet_record_replay_pending();
    let result = observe_rescan(&mut db_conn, bit).and_then(|rescan| {
        // Keep the persisted rescan marker until the post-rollback wallet
        // update succeeds. Otherwise callers can observe completion while
        // coins are still unconfirmed and recovery is temporarily unavailable.
        if let RescanObservation::Completed(tip) = rescan {
            db_conn.rollback_tip(&tip);
            updates(&mut db_conn, bit, descs, secp, reorg_alert)?;
            db_conn.complete_rescan();
            return Ok(());
        }
        if let RescanObservation::Running(progress) = rescan {
            log::info!("Rescan progress: {:.2}%.", progress * 100.0);
        }
        updates(&mut db_conn, bit, descs, secp, reorg_alert)
    });
    let now: u32 = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .expect("current system time must be later than epoch")
        .as_secs()
        .try_into()
        .expect("system clock year is earlier than 2106");
    if let Err(error) = result {
        let (failure, changed) = history_sync.poll_failed(&error, now);
        if failure_is_worth_a_warning(&failure, changed) {
            log::warn!(
                "Poll failed ({} in a row): {}. The wallet keeps its last known state \
                 and the next poll retries.",
                failure.consecutive,
                error
            );
        } else {
            log::debug!(
                "Poll deferred without updating successful-poll time: {}",
                error
            );
        }
        return;
    }
    if let Some(ticket) = replay_ticket {
        bit.acknowledge_wallet_record_replay(ticket);
    }
    if !save_imported_scan_completion(&mut db_conn, history_sync, now) {
        return;
    }
    history_sync.poll_succeeded(now);
    db_conn.set_last_poll(now);
}

fn save_imported_scan_completion(
    db_conn: &mut Box<dyn DatabaseConnection>,
    history_sync: &HistorySyncCache,
    now: u32,
) -> bool {
    if let Some((receive_scanned, change_scanned)) = history_sync.pending_full_scan_coverage() {
        if db_conn.imported_history().is_some() {
            if let Err(error) = db_conn.complete_import_history(receive_scanned, change_scanned) {
                let (failure, changed) = history_sync.poll_failed(&error, now);
                if failure_is_worth_a_warning(&failure, changed) {
                    log::warn!("Imported history status could not be saved: {error}");
                }
                return false;
            }
        }
    }
    true
}

/// Whether a failed poll should be logged as a warning rather than at debug level.
///
/// Failures used to be debug-only, which made a wallet that could not sync for hours
/// look healthy in the logs. Logging every one at warning level would instead flood
/// them — the poller retries every few seconds — and a single failure is routine (the
/// tip moved mid-poll, a server hiccupped) and recovers on the next poll. So warn once
/// failures repeat, whenever a repeating failure changes, and every
/// [`REPEATED_FAILURE_WARN_INTERVAL`] repeats after that.
fn failure_is_worth_a_warning(failure: &PollFailure, changed: bool) -> bool {
    failure.consecutive >= 2
        && (changed
            || failure.consecutive == 2
            || failure
                .consecutive
                .is_multiple_of(REPEATED_FAILURE_WARN_INTERVAL))
}

/// See [`failure_is_worth_a_warning`].
const REPEATED_FAILURE_WARN_INTERVAL: u32 = 10;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bitcoin::ChainAlert,
        database::{BlockInfo, Coin, DatabaseInterface},
        testutils::{DummyBitcoind, DummyDatabase},
    };
    use std::str::FromStr;

    use miniscript::{
        bitcoin::{bip32, hashes::Hash},
        descriptor,
    };

    #[test]
    fn genesis_failure_preserves_uninitialized_tip_and_recovers_on_retry() {
        use crate::{bitcoin::GenesisError, connect::AdmissionError, StartupError};
        for error in [
            AdmissionError::Unavailable,
            AdmissionError::Throttled,
            AdmissionError::IndexerBehind,
        ] {
            let db = DummyDatabase::new();
            let mut bit = DummyBitcoind::new();
            bit.genesis_error = Some(error);
            let result = maybe_initialize_tip(&bit, &db).unwrap_err();
            assert!(matches!(&result, GenesisError::Esplora(_)));
            assert!(
                matches!(StartupError::from(result), StartupError::ConnectAdmission(actual) if actual == error)
            );
            assert!(db.connection().chain_tip().is_none());
            bit.genesis_error = None;
            maybe_initialize_tip(&bit, &db).unwrap();
            assert_eq!(
                db.connection().chain_tip(),
                Some(bit.genesis_block().unwrap())
            );
            // An initialized wallet need not reach a temporarily failed backend.
            bit.genesis_error = Some(error);
            maybe_initialize_tip(&bit, &db).unwrap();
            assert_eq!(db.connection().chain_tip().unwrap().height, 0);
        }
    }

    fn tip(height: i32, seed: u8) -> BlockChainTip {
        BlockChainTip {
            hash: bitcoin::BlockHash::from_byte_array([seed; 32]),
            height,
        }
    }

    /// A backend whose chain forks `depth` blocks below our tip.
    fn forked_backend(our_tip: &BlockChainTip, depth: i32) -> DummyBitcoind {
        let mut bit = DummyBitcoind::new();
        // A different block at the same height: a reorg, not forward progress.
        bit.tip = tip(our_tip.height, 0xbb);
        bit.in_chain = false;
        bit.ancestor = Some(tip(our_tip.height - depth, 0xcc));
        bit
    }

    #[test]
    fn shallow_reorg_is_applied() {
        let our_tip = tip(20_000, 0xaa);
        let bit = forked_backend(&our_tip, MAX_REORG_DEPTH);
        match new_tip(&bit, &our_tip) {
            TipUpdate::Reorged(ancestor) => {
                assert_eq!(ancestor.height, 20_000 - MAX_REORG_DEPTH);
            }
            other => panic!("expected a reorg at the depth limit, got {:?}", other),
        }
    }

    #[test]
    fn deep_reorg_is_refused() {
        let our_tip = tip(20_000, 0xaa);
        // One block past the limit is already refused, and is still inside the
        // ancestor walk's bound, so the depth is known exactly.
        let bit = forked_backend(&our_tip, MAX_REORG_DEPTH + 1);
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { min_depth } => {
                assert_eq!(min_depth, MAX_REORG_DEPTH + 1)
            }
            other => panic!("expected the reorg to be refused, got {:?}", other),
        }

        // And so is a 10k-block rewind, the shape a deep `invalidateblock` on the
        // node would produce. Here the walk stops at its bound rather than paying for
        // 10k round-trips, so we know only that the fork is at least that deep —
        // and crucially this must NOT come back as a failed lookup to be retried.
        let bit = forked_backend(&our_tip, 10_000);
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { min_depth } => {
                assert_eq!(min_depth, MAX_REORG_DEPTH + 1)
            }
            other => panic!("expected the deep reorg to be refused, got {:?}", other),
        }
    }

    // The bound is what stops a misreporting backend from extracting one round-trip
    // per claimed block from us before we reject its answer.
    #[test]
    fn the_ancestor_walk_is_bounded() {
        use crate::bitcoin::AncestorSearch;

        let our_tip = tip(20_000, 0xaa);
        let bit = forked_backend(&our_tip, 10_000);
        assert_eq!(
            bit.common_ancestor(&our_tip, MAX_REORG_DEPTH + 1),
            AncestorSearch::TooDeep
        );
        // Within the bound the ancestor is still found normally.
        let shallow = forked_backend(&our_tip, 10);
        assert_eq!(
            shallow.common_ancestor(&our_tip, MAX_REORG_DEPTH + 1),
            AncestorSearch::Found(tip(19_990, 0xcc))
        );

        // A walk starts at the tip and only moves down, so no real backend can hand
        // back an ancestor *above* it. The double refuses to model one: left to fall
        // through, the depth would compute as zero, read as a shallow reorg, and roll
        // our tip forward — a scenario production can never actually be shown.
        let mut inverted = forked_backend(&our_tip, 10);
        inverted.ancestor = Some(tip(our_tip.height + 1, 0xdd));
        assert_eq!(
            inverted.common_ancestor(&our_tip, MAX_REORG_DEPTH + 1),
            AncestorSearch::Failed
        );
    }

    #[test]
    fn failed_ancestor_lookup_gives_up_instead_of_spinning() {
        let our_tip = tip(20_000, 0xaa);
        let mut bit = forked_backend(&our_tip, 10);
        // The lookup never succeeds. Before the retry bound this re-entered `new_tip`
        // forever, hammering the backend and growing the stack.
        bit.ancestor = None;
        assert!(matches!(new_tip(&bit, &our_tip), TipUpdate::Unavailable));
    }

    /// A backend that has forked away from our tip and cannot be asked where: the
    /// shape an Esplora or Electrum Vault has on every poll after a refused reorg.
    fn diverged_backend(our_tip: &BlockChainTip, backend_height: i32) -> DummyBitcoind {
        let mut bit = forked_backend(our_tip, 0);
        bit.tip = tip(backend_height, 0xbb);
        bit.walks_ancestors = false;
        // Whatever it might say, it is never consulted; a fixture that answers proves
        // less than one that would be wrong if it were.
        bit.ancestor = Some(tip(0, 0xcc));
        bit
    }

    // The regression: our tip diverged from an Esplora backend's, which is exactly what
    // refusing a reorg leaves behind. Asking it for the fork point hit an
    // `unreachable!()`, and with `panic = "abort"` that killed the whole desktop app —
    // observed in the field as a crash one second after the refusal, mid-transaction.
    #[test]
    fn diverged_backend_is_reported_not_asked() {
        let our_tip = tip(146_244, 0xaa);

        // Rewound all the way to genesis: the depth that got refused in the field.
        match new_tip(&diverged_backend(&our_tip, 0), &our_tip) {
            TipUpdate::Diverged { backend_tip } => assert_eq!(backend_tip.height, 0),
            other => panic!("expected the divergence to be reported, got {:?}", other),
        }

        // Same height, different block. Also a divergence, and one where the depth is
        // not a rollback at all.
        match new_tip(&diverged_backend(&our_tip, our_tip.height), &our_tip) {
            TipUpdate::Diverged { .. } => {}
            other => panic!("expected the divergence to be reported, got {:?}", other),
        }

        // A backend ahead of us that cannot vouch for our tip either. Not forward
        // progress — we must not record blocks from a chain our own tip isn't on.
        match new_tip(&diverged_backend(&our_tip, our_tip.height + 10), &our_tip) {
            TipUpdate::Diverged { .. } => {}
            other => panic!("expected the divergence to be reported, got {:?}", other),
        }

        // And a backend that *can* walk must still be asked, rather than every reorg
        // being written off as a divergence.
        assert!(matches!(
            new_tip(&forked_backend(&our_tip, 3), &our_tip),
            TipUpdate::Reorged(_)
        ));
    }

    #[test]
    fn unchanged_tip_is_still_reported_as_same() {
        let our_tip = tip(100, 0xaa);
        let mut bit = DummyBitcoind::new();
        bit.tip = our_tip;
        assert!(matches!(new_tip(&bit, &our_tip), TipUpdate::Same));
    }

    // The maintenance flag exists so a deliberate rewind doesn't make every Vault
    // flap into a warning state. It must scope to bitcoind-backed Vaults only: an
    // Esplora or Electrum Vault has its own view of the chain and is unaffected by
    // whatever we do to the managed node, so pausing it would be a plain bug.
    #[test]
    fn maintenance_pauses_only_bitcoind_backends() {
        use crate::bitcoin::{
            managed_node_maintenance, set_managed_node_maintenance, BitcoinInterface,
            MaintenanceGuard,
        };

        assert!(!managed_node_maintenance());
        // `DummyBitcoind` stands in for a non-bitcoind backend: it takes the trait's
        // default `is_bitcoind() == false`.
        assert!(!DummyBitcoind::new().is_bitcoind());

        {
            let guard = MaintenanceGuard::try_acquire().expect("uncontended acquire");
            assert!(managed_node_maintenance());
            // Mutual exclusion: several Vaults attach to the one shared node, and only
            // one may rewind it. A check-then-set would let two through here.
            assert!(
                MaintenanceGuard::try_acquire().is_none(),
                "a second holder must not be able to claim maintenance"
            );
            drop(guard);
        }
        // The guard must clear on drop — a flag left set silently stops every Vault
        // from ever updating again.
        assert!(!managed_node_maintenance());
        // And release must make it claimable again.
        assert!(MaintenanceGuard::try_acquire().is_some());
        assert!(!managed_node_maintenance());

        set_managed_node_maintenance(true);
        set_managed_node_maintenance(false);
        assert!(!managed_node_maintenance());
    }

    pub(super) fn test_descs() -> [miniscript::Descriptor<miniscript::DescriptorPublicKey>; 2] {
        let owner_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap());
        let heir_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap());
        let policy = descriptors::CoincubePolicy::new_legacy(
            owner_key,
            [(10u16, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);
        [
            desc.receive_descriptor().as_descriptor_public_key().clone(),
            desc.change_descriptor().as_descriptor_public_key().clone(),
        ]
    }

    /// The acceptance test for this guard: a 10k-block rewind — what a deep
    /// `invalidateblock` on the node looks like to the poller — must not roll our tip
    /// back and must not delete a single coin row.
    #[test]
    fn deep_reorg_leaves_coins_and_tip_untouched() {
        let our_tip = tip(20_000, 0xaa);

        let mut db = DummyDatabase::new();
        db.connection().update_tip(&our_tip);
        let outpoint = bitcoin::OutPoint::from_str(
            "3d8ea3e05e4c1e2f4b2e9dbd4a2b4e0dd7f6b0f7c9e8d5a4b3c2d1e0f9a8b7c6:0",
        )
        .unwrap();
        db.insert_coins(vec![Coin {
            outpoint,
            is_immature: false,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from_normal_idx(0).unwrap(),
            is_change: false,
            block_info: None,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);

        let mut bit = forked_backend(&our_tip, 10_000);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert!(
            db.rollbacks().is_empty(),
            "a {}-block reorg must not roll our tip back",
            10_000
        );
        assert_eq!(
            db.coin_outpoints(),
            vec![outpoint],
            "no coin may be removed on a refused reorg"
        );
        assert_eq!(db_conn.chain_tip(), Some(our_tip), "our tip must not move");
        // A lower bound, not the true 10,000: the ancestor walk stops at its limit
        // rather than paying for a round-trip per block just to report a number.
        assert_eq!(
            alert.load(),
            ChainAlert::RefusedReorg(MAX_REORG_DEPTH + 1),
            "the refusal must be published for get_info"
        );
    }

    /// A wallet holding a single unconfirmed coin, its stored tip at `our_tip`. The
    /// shared fixture for the divergence end-to-end tests below.
    fn wallet_with_one_coin(our_tip: &BlockChainTip) -> (DummyDatabase, bitcoin::OutPoint) {
        let mut db = DummyDatabase::new();
        db.connection().update_tip(our_tip);
        let outpoint = bitcoin::OutPoint::from_str(
            "3d8ea3e05e4c1e2f4b2e9dbd4a2b4e0dd7f6b0f7c9e8d5a4b3c2d1e0f9a8b7c6:0",
        )
        .unwrap();
        db.insert_coins(vec![Coin {
            outpoint,
            is_immature: false,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from_normal_idx(0).unwrap(),
            is_change: false,
            block_info: None,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        (db, outpoint)
    }

    /// A full poll against a backend our tip has diverged from must skip with wallet
    /// state intact and surface an *explicit* divergence for `get_info` — never a bogus
    /// rollback depth. The alert cache is fresh, exactly as after a daemon restart: the
    /// divergence has to be re-derived from the backend, not recovered from a surviving
    /// in-memory alert.
    fn assert_divergence_reported(our_tip: BlockChainTip, mut bit: DummyBitcoind) {
        let (db, outpoint) = wallet_with_one_coin(&our_tip);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert!(
            db.rollbacks().is_empty(),
            "our tip must not be rolled back while diverged"
        );
        assert_eq!(
            db.coin_outpoints(),
            vec![outpoint],
            "no coin may be removed while diverged"
        );
        assert_eq!(
            db_conn.chain_tip(),
            Some(our_tip),
            "our tip must not move while diverged"
        );
        assert_eq!(
            alert.load(),
            ChainAlert::Diverged,
            "an unresolved divergence must be published for get_info as a divergence, not \
             misreported as a refused-reorg depth"
        );
    }

    /// The end-to-end shape of the crash: a Vault whose tip is off its Esplora backend's
    /// chain, the backend rewound all the way below our tip (to genesis, the depth that
    /// got refused in the field). It must skip the poll with its state intact, not abort
    /// the process — and now report the divergence explicitly rather than dressing the
    /// height gap up as an exact rollback depth.
    #[test]
    fn diverged_backend_leaves_coins_and_tip_untouched() {
        let our_tip = tip(146_244, 0xaa);
        assert_divergence_reported(our_tip, diverged_backend(&our_tip, 0));
    }

    /// Backend at the *same* height as us but on a different chain. The tips' height gap
    /// is zero, so the old height-based heuristic stored nothing and `get_info` fell
    /// silent even though sync was deliberately paused.
    #[test]
    fn diverged_backend_at_same_height_is_reported() {
        let our_tip = tip(146_244, 0xaa);
        assert_divergence_reported(our_tip, diverged_backend(&our_tip, our_tip.height));
    }

    /// Backend *ahead* of us but not containing our tip. Not forward progress — recording
    /// its blocks would append to a chain our own tip isn't on — and, like the same-height
    /// case, a negative height gap the old heuristic read as "no alert".
    #[test]
    fn diverged_backend_above_our_tip_is_reported() {
        let our_tip = tip(146_244, 0xaa);
        assert_divergence_reported(our_tip, diverged_backend(&our_tip, our_tip.height + 10));
    }

    /// A daemon restart drops the in-memory alert. The next poll must re-derive the
    /// divergence from the backend rather than depend on a surviving alert — the exact
    /// regression, since a restart mid-divergence against a same-height backend otherwise
    /// left `get_info` reporting "no alert" for a poll that is in fact paused.
    #[test]
    fn divergence_surfaces_from_a_fresh_cache_after_restart() {
        let our_tip = tip(146_244, 0xaa);
        let (db, _outpoint) = wallet_with_one_coin(&our_tip);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        // A just-restarted daemon holds no prior alert in memory.
        let alert = ReorgAlertCache::default();
        assert_eq!(
            alert.load(),
            ChainAlert::None,
            "a fresh daemon starts with no alert"
        );
        let mut db_conn = db.connection();

        // Backend at our height on a different chain: the zero-gap case the old heuristic
        // recorded as "no alert".
        let mut bit = diverged_backend(&our_tip, our_tip.height);
        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert_eq!(
            alert.load(),
            ChainAlert::Diverged,
            "divergence must resurface from the backend after a restart, not depend on an \
             in-memory alert that a restart erased"
        );
        assert_eq!(
            db_conn.chain_tip(),
            Some(our_tip),
            "our tip must not move while diverged"
        );
    }

    /// The divergence status clears only once a poll succeeds and sync resumes — i.e.
    /// when the backend's chain contains our tip again.
    #[test]
    fn divergence_clears_once_the_chains_converge() {
        let our_tip = tip(146_244, 0xaa);
        let (db, outpoint) = wallet_with_one_coin(&our_tip);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        // First poll: diverged. The status is published and wallet state is held.
        let mut bit = diverged_backend(&our_tip, our_tip.height + 5);
        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);
        assert_eq!(alert.load(), ChainAlert::Diverged);
        assert_eq!(
            db_conn.chain_tip(),
            Some(our_tip),
            "state is held while diverged"
        );

        // The provider recovers: its chain now contains our tip and simply extends it, so
        // the next poll takes the ordinary forward-progress path and resumes syncing.
        bit.in_chain = true;
        bit.tip = tip(our_tip.height + 5, 0xaa);
        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert_eq!(
            alert.load(),
            ChainAlert::None,
            "the divergence status must clear once the chains converge and sync resumes"
        );
        assert_eq!(
            db_conn.chain_tip(),
            Some(tip(our_tip.height + 5, 0xaa)),
            "sync resumes: our tip advances onto the reconverged chain"
        );
        assert_eq!(
            db.coin_outpoints(),
            vec![outpoint],
            "the coin survives convergence"
        );
    }

    /// An Electrum/Esplora-like backend whose next sync reports `ancestor` as the common
    /// ancestor of a reorg, the way those backends hand it over.
    fn backend_reporting_ancestor(ancestor: BlockChainTip) -> DummyBitcoind {
        let mut bit = DummyBitcoind::new();
        bit.walks_ancestors = false;
        bit.sync_reorg_ancestor = Some(ancestor);
        bit
    }

    fn confirmed_coin(seed: u8, height: i32, spend_height: Option<i32>) -> Coin {
        let block = |height| BlockInfo { height, time: 0 };
        Coin {
            outpoint: bitcoin::OutPoint::new(bitcoin::Txid::from_byte_array([seed; 32]), 0),
            is_immature: false,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from_normal_idx(0).unwrap(),
            is_change: false,
            block_info: Some(block(height)),
            spend_txid: spend_height.map(|_| bitcoin::Txid::from_byte_array([seed ^ 0xff; 32])),
            spend_block: spend_height.map(block),
            is_from_self: false,
        }
    }

    /// Poll once against `bit` with `coins` stored and our tip at `our_tip`.
    fn poll_reported_reorg(
        our_tip: BlockChainTip,
        coins: Vec<Coin>,
        mut bit: DummyBitcoind,
    ) -> (DummyDatabase, ReorgAlertCache) {
        let mut db = DummyDatabase::new();
        db.connection().update_tip(&our_tip);
        db.insert_coins(coins);
        let alert = ReorgAlertCache::default();
        let _ = updates(
            &mut db.connection(),
            &mut bit,
            &test_descs(),
            &secp256k1::Secp256k1::verification_only(),
            &alert,
        );
        (db, alert)
    }

    /// From the field: a testnet4 Vault last synced onto a block that was then orphaned.
    /// After a restart its Esplora backend's nearest shared block was where its last coin
    /// confirmed, 1,058 blocks down. That gap was refused as a 1,058-block reorg, and the
    /// wallet stopped syncing for good: it missed a confirmed spend of its only coin and
    /// later offered that coin for a send. Nothing above the ancestor is confirmed, so the
    /// rollback undoes nothing and must be applied.
    #[test]
    fn stale_tip_far_above_the_reported_ancestor_rolls_back() {
        let our_tip = tip(156_754, 0xaa);
        let ancestor = tip(155_696, 0xcc);
        let coin = confirmed_coin(1, 155_696, None);

        let (db, alert) =
            poll_reported_reorg(our_tip, vec![coin], backend_reporting_ancestor(ancestor));

        assert_eq!(db.rollbacks(), vec![ancestor]);
        assert_eq!(db.coin_outpoints(), vec![coin.outpoint]);
        assert_eq!(alert.load(), ChainAlert::None);
    }

    /// From the field: a wallet with no confirmed coins keeps only genesis below its tip,
    /// so a stale tip was reported as a reorg of the whole chain.
    #[test]
    fn stale_tip_of_a_wallet_without_confirmed_coins_rolls_back_to_genesis() {
        let our_tip = tip(158_742, 0xaa);
        let genesis = tip(0, 0xcc);

        let (db, alert) = poll_reported_reorg(our_tip, vec![], backend_reporting_ancestor(genesis));

        assert_eq!(db.rollbacks(), vec![genesis]);
        assert_eq!(alert.load(), ChainAlert::None);
    }

    /// The guard still holds where it matters: a rollback that would undo a confirmation
    /// deeper than the limit is refused, and is measured by that confirmation, not by the
    /// gap to the ancestor. A spend counts as much as a deposit.
    #[test]
    fn reported_reorg_undoing_a_deep_confirmation_is_refused() {
        let our_tip = tip(20_000, 0xaa);
        let ancestor = tip(17_000, 0xcc);
        // Deposit below the ancestor (untouched), spend 1,001 blocks deep (undone).
        let deep_spend = confirmed_coin(1, 16_000, Some(19_000));
        // Undone too, but shallower: the deepest confirmation sets the depth.
        let shallow = confirmed_coin(2, 19_900, None);

        let (db, alert) = poll_reported_reorg(
            our_tip,
            vec![deep_spend, shallow],
            backend_reporting_ancestor(ancestor),
        );

        assert!(db.rollbacks().is_empty(), "the rollback must be refused");
        let mut outpoints = db.coin_outpoints();
        outpoints.sort();
        let mut expected = vec![deep_spend.outpoint, shallow.outpoint];
        expected.sort();
        assert_eq!(outpoints, expected, "no coin may be removed");
        assert_eq!(alert.load(), ChainAlert::RefusedReorg(1_001));
    }

    /// A reported rollback that undoes confirmations within the limit is applied.
    #[test]
    fn reported_reorg_undoing_a_shallow_confirmation_rolls_back() {
        let our_tip = tip(20_000, 0xaa);
        let ancestor = tip(17_000, 0xcc);
        let coin = confirmed_coin(1, 20_000 - MAX_REORG_DEPTH + 1, None);

        let (db, alert) =
            poll_reported_reorg(our_tip, vec![coin], backend_reporting_ancestor(ancestor));

        assert_eq!(db.rollbacks(), vec![ancestor]);
        assert_eq!(alert.load(), ChainAlert::None);
    }

    #[test]
    fn shallow_reorg_still_rolls_back() {
        // The guard must not block ordinary reorgs.
        let our_tip = tip(20_000, 0xaa);
        let db = DummyDatabase::new();
        db.connection().update_tip(&our_tip);

        let mut bit = forked_backend(&our_tip, 3);
        // Once we've rolled back, stop reporting a fork so `updates` can finish.
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        // After the rollback the recursive call sees a tip equal to the rolled-back one.
        bit.tip = tip(20_000 - 3, 0xcc);
        let _ = updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert_eq!(db.rollbacks(), vec![tip(20_000 - 3, 0xcc)]);
        assert_eq!(
            alert.load(),
            ChainAlert::None,
            "an applied reorg raises no alert"
        );
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use crate::testutils::{DummyBitcoind, DummyDatabase};
    use bitcoin::{bip32::ChildNumber, hashes::Hash};

    #[test]
    fn an_old_poll_cannot_acknowledge_a_replay_requested_after_its_record_read() {
        let state = sync::Arc::new(sync::Mutex::new(
            crate::bitcoin::WalletRecordReplay::default(),
        ));
        let mut backend = DummyBitcoind::new();
        backend.record_replay = Some(state.clone());
        backend.request_replay_during_received = true;
        let db = DummyDatabase::new();
        db.connection().update_tip(&backend.tip);
        let mut bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>> =
            sync::Arc::new(sync::Mutex::new(backend));
        let db: sync::Arc<sync::Mutex<dyn DatabaseInterface>> =
            sync::Arc::new(sync::Mutex::new(db));
        poll(
            &mut bit,
            &db,
            &secp256k1::Secp256k1::verification_only(),
            &super::tests::test_descs(),
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(state.lock().unwrap().pending(), Some(2));
        assert!(db.connection().last_poll_timestamp().is_some());
    }

    #[test]
    fn startup_replay_outlier_does_not_stall_polling_but_explicit_recovery_stays_incomplete() {
        let secp = secp256k1::Secp256k1::verification_only();
        let descs = super::tests::test_descs();
        for explicit in [false, true] {
            let state = sync::Arc::new(sync::Mutex::new(
                crate::bitcoin::WalletRecordReplay::default(),
            ));
            if explicit {
                state.lock().unwrap().request().unwrap();
            }
            let mut backend = DummyBitcoind::new();
            backend.record_replay = Some(state.clone());
            let database = DummyDatabase::new();
            database.connection().update_tip(&backend.tip);
            backend.received = vec![UTxO {
                outpoint: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), 0),
                amount: bitcoin::Amount::from_sat(100_000),
                block_height: Some(99),
                address: UTxOAddress::Address(
                    descs[0]
                        .at_derivation_index(1500)
                        .unwrap()
                        .address(database.connection().network())
                        .unwrap()
                        .into_unchecked(),
                ),
                is_immature: false,
            }];
            let mut bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>> =
                sync::Arc::new(sync::Mutex::new(backend));
            let db: sync::Arc<sync::Mutex<dyn DatabaseInterface>> =
                sync::Arc::new(sync::Mutex::new(database));
            let history = HistorySyncCache::default();
            poll(&mut bit, &db, &secp, &descs, &Default::default(), &history);
            assert_eq!(
                state.lock().unwrap().pending(),
                if explicit { Some(2) } else { None }
            );
            assert_eq!(db.connection().last_poll_timestamp().is_some(), !explicit);
            if !explicit {
                // A second ordinary poll can advance the chain after the startup outlier.
                poll(&mut bit, &db, &secp, &descs, &Default::default(), &history);
                assert!(db.connection().last_poll_timestamp().is_some());
            }
        }
    }

    #[test]
    fn staged_lookahead_resolves_reversed_deposits_and_skips_unmapped_without_writes() {
        let secp = secp256k1::Secp256k1::verification_only();
        let descs = super::tests::test_descs();
        for branch in [0, 1] {
            let db = DummyDatabase::new();
            let mut conn = db.connection();
            let mut bit = DummyBitcoind::new();
            let first = UTxO {
                outpoint: bitcoin::OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout: 0,
                },
                amount: bitcoin::Amount::from_sat(100_000),
                block_height: Some(99),
                address: UTxOAddress::DerivIndex(199.into(), branch == 1),
                is_immature: false,
            };
            let next = UTxO {
                outpoint: bitcoin::OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout: 1,
                },
                address: UTxOAddress::Address(
                    descs[branch]
                        .at_derivation_index(200)
                        .unwrap()
                        .address(conn.network())
                        .unwrap()
                        .into_unchecked(),
                ),
                ..first.clone()
            };
            let extended = UTxO {
                outpoint: bitcoin::OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout: 2,
                },
                address: UTxOAddress::Address(
                    descs[branch]
                        .at_derivation_index(399)
                        .unwrap()
                        .address(conn.network())
                        .unwrap()
                        .into_unchecked(),
                ),
                ..first.clone()
            };
            let unmapped = UTxO {
                outpoint: bitcoin::OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout: 3,
                },
                address: UTxOAddress::Address(
                    descs[branch]
                        .at_derivation_index(1000)
                        .unwrap()
                        .address(conn.network())
                        .unwrap()
                        .into_unchecked(),
                ),
                ..first.clone()
            };
            // Three passes are needed: 199 unlocks 200, which unlocks 399.
            // The distant outlier must not prevent unrelated deposits updating.
            bit.received = vec![extended, next, first, unmapped];
            let staged = update_coins(&bit, &mut conn, &bit.tip, &descs, &secp).unwrap();
            let mut indexes: Vec<_> = staged
                .received
                .iter()
                .map(|coin| u32::from(coin.derivation_index))
                .collect();
            indexes.sort_unstable();
            assert_eq!(indexes, vec![199, 200, 399]);
            assert!(staged
                .received
                .iter()
                .all(|coin| coin.is_change == (branch == 1)));
            assert_eq!(
                if branch == 1 {
                    staged.change_index
                } else {
                    staged.receive_index
                },
                ChildNumber::from(399)
            );
            assert_eq!(conn.receive_index(), ChildNumber::from(0));
            assert_eq!(conn.change_index(), ChildNumber::from(0));
            bit.poll_failure = Some("confirmed");
            assert!(update_coins(&bit, &mut conn, &bit.tip, &descs, &secp).is_err());
            assert_eq!(conn.receive_index(), ChildNumber::from(0));
            assert_eq!(conn.change_index(), ChildNumber::from(0));
            assert!(conn.coins(&[], &[]).is_empty());
        }
    }

    #[test]
    fn failed_poll_preserves_saved_state_and_success_timestamp() {
        let descs = super::tests::test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        for failure in ["confirmed", "rescan"] {
            let mut bit = DummyBitcoind::new();
            bit.poll_failure = Some(failure);
            let db = DummyDatabase::new();
            let mut conn = db.connection();
            conn.update_tip(&bit.tip);
            conn.set_last_poll(123);
            let tip = bit.tip;
            let mut backend: sync::Arc<sync::Mutex<dyn BitcoinInterface>> =
                sync::Arc::new(sync::Mutex::new(bit));
            let database: sync::Arc<sync::Mutex<dyn DatabaseInterface>> =
                sync::Arc::new(sync::Mutex::new(db));
            let alert = ReorgAlertCache::default();
            poll(
                &mut backend,
                &database,
                &secp,
                &descs,
                &alert,
                &HistorySyncCache::default(),
            );
            assert_eq!(conn.chain_tip(), Some(tip));
            assert_eq!(conn.last_poll_timestamp(), Some(123));
            assert!(conn.coins(&[], &[]).is_empty());
        }
    }
}

#[cfg(test)]
mod rescan_completion_tests {
    use super::*;
    use crate::testutils::{DummyBitcoind, DummyDatabase};

    #[test]
    fn rescan_stays_pending_until_post_rollback_update_succeeds() {
        let mut bit = DummyBitcoind::new();
        let tip = bit.tip;
        bit.rescan_start = Some(BlockChainTip { height: 90, ..tip });
        bit.poll_failure = Some("confirmed");
        let bit = sync::Arc::new(sync::Mutex::new(bit));
        let mut backend: sync::Arc<sync::Mutex<dyn BitcoinInterface>> = bit.clone();
        let db = DummyDatabase::new();
        let mut conn = db.connection();
        conn.update_tip(&tip);
        conn.set_rescan(1000);
        conn.set_last_poll(123);
        let database: sync::Arc<sync::Mutex<dyn DatabaseInterface>> =
            sync::Arc::new(sync::Mutex::new(db));
        let secp = secp256k1::Secp256k1::verification_only();
        let descs = super::tests::test_descs();
        let alert = ReorgAlertCache::default();
        poll(
            &mut backend,
            &database,
            &secp,
            &descs,
            &alert,
            &HistorySyncCache::default(),
        );
        assert_eq!(conn.rescan_timestamp(), Some(1000));
        assert_eq!(conn.last_poll_timestamp(), Some(123));
        assert_eq!(conn.chain_tip().unwrap().height, 90);
        bit.lock().unwrap().poll_failure = None;
        poll(
            &mut backend,
            &database,
            &secp,
            &descs,
            &alert,
            &HistorySyncCache::default(),
        );
        assert_eq!(conn.rescan_timestamp(), None);
        assert_eq!(conn.chain_tip(), Some(tip));
        assert_ne!(conn.last_poll_timestamp(), Some(123));
    }
}

#[cfg(test)]
mod history_sync_tests {
    use super::*;
    use crate::testutils::{DummyBitcoind, DummyDatabase};

    struct Harness {
        bit: sync::Arc<sync::Mutex<DummyBitcoind>>,
        backend: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        database: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        descs: Vec<miniscript::Descriptor<miniscript::DescriptorPublicKey>>,
        secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
        alert: ReorgAlertCache,
        history: HistorySyncCache,
    }

    impl Harness {
        fn new() -> Self {
            let bit = DummyBitcoind::new();
            let db = DummyDatabase::new();
            db.connection().update_tip(&bit.tip);
            let bit = sync::Arc::new(sync::Mutex::new(bit));
            Harness {
                backend: bit.clone(),
                bit,
                database: sync::Arc::new(sync::Mutex::new(db)),
                descs: super::tests::test_descs().to_vec(),
                secp: secp256k1::Secp256k1::verification_only(),
                alert: ReorgAlertCache::default(),
                history: HistorySyncCache::default(),
            }
        }

        fn fail_polls(&self, failure: Option<&'static str>) {
            self.bit.lock().unwrap().poll_failure = failure;
        }

        fn poll(&mut self) {
            poll(
                &mut self.backend,
                &self.database,
                &self.secp,
                &self.descs,
                &self.alert,
                &self.history,
            );
        }
    }

    /// Failed polls used to be logged at debug level and recorded nowhere, so a Vault
    /// that could not sync for hours looked healthy to the app. Each one now extends a
    /// failure run `get_info` reports, and the first successful poll ends it.
    #[test]
    fn failed_polls_are_recorded_until_a_poll_succeeds() {
        let mut h = Harness::new();
        h.fail_polls(Some("confirmed"));
        h.poll();
        let failure = h
            .history
            .snapshot()
            .last_failure
            .expect("first failure recorded");
        assert_eq!(failure.consecutive, 1);
        assert_eq!(failure.since, failure.last_at);
        h.poll();
        let failure = h.history.snapshot().last_failure.expect("run continues");
        assert_eq!(failure.consecutive, 2);

        h.fail_polls(None);
        h.poll();
        assert_eq!(h.history.snapshot().last_failure, None);
        assert!(h.database.connection().last_poll_timestamp().is_some());
    }

    /// A full scan is only complete once the poll carrying it has written it to the
    /// database. A scan the backend fetched, followed by a poll that failed further
    /// on, is still owed its commit — and the next successful poll delivers it.
    #[test]
    fn a_fetched_full_scan_completes_with_the_first_successful_poll() {
        let mut h = Harness::new();
        h.history.begin_full_scan(402);
        h.history.address_checked();
        h.history.full_scan_fetched();
        h.fail_polls(Some("confirmed"));
        h.poll();
        let snapshot = h.history.snapshot();
        assert!(!snapshot.full_scan_in_progress);
        assert_eq!(snapshot.full_scan_completed_at, None);

        h.fail_polls(None);
        h.poll();
        let snapshot = h.history.snapshot();
        assert!(snapshot.full_scan_completed_at.is_some());
        assert_eq!(snapshot.last_failure, None);
    }

    /// A poll that fails mid-scan loses that scan: the backend starts over next time.
    /// The record must not go on reporting it as running.
    #[test]
    fn a_failed_poll_ends_the_scan_it_was_running() {
        let mut h = Harness::new();
        h.history.begin_full_scan(402);
        for _ in 0..50 {
            h.history.address_checked();
        }
        h.fail_polls(Some("confirmed"));
        h.poll();
        let snapshot = h.history.snapshot();
        assert!(!snapshot.full_scan_in_progress);
        assert_eq!(snapshot.full_scan_progress(), None);
        assert_eq!(snapshot.full_scan_completed_at, None);
    }

    #[test]
    fn repeated_failures_warn_once_they_repeat_on_changes_and_periodically() {
        let failure = |consecutive| PollFailure {
            message: "Esplora client error".into(),
            since: 1,
            last_at: 2,
            consecutive,
        };
        // One failure is routine and recovers on the next poll.
        assert!(!failure_is_worth_a_warning(&failure(1), true));
        assert!(failure_is_worth_a_warning(&failure(2), false));
        assert!(!failure_is_worth_a_warning(&failure(3), false));
        assert!(failure_is_worth_a_warning(&failure(7), true));
        assert!(!failure_is_worth_a_warning(&failure(9), false));
        assert!(failure_is_worth_a_warning(
            &failure(REPEATED_FAILURE_WARN_INTERVAL),
            false
        ));
    }
}

#[cfg(test)]
mod coinbase_maturity_tests {
    //! The poller applies the chain's coinbase maturity on top of the backends'
    //! 100-block floor (`hold_immature_coinbase`).
    use super::*;
    use crate::{
        database::{BlockInfo, DatabaseInterface},
        testutils::{DummyBitcoind, DummyDatabase},
    };
    use coincube_core::chain::ChainId;
    use miniscript::bitcoin::{
        absolute, bip32::ChildNumber, transaction, Amount, ScriptBuf, Sequence, Transaction, TxIn,
        TxOut, Witness,
    };

    /// A coinbase transaction, made unique by `seed`.
    fn coinbase_tx(seed: u8) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: bitcoin::OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![1, seed]),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(312_500_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    fn coin(outpoint: bitcoin::OutPoint, block_height: Option<i32>, is_immature: bool) -> Coin {
        Coin {
            outpoint,
            is_immature,
            block_info: block_height.map(|height| BlockInfo { height, time: 1 }),
            amount: Amount::from_sat(312_500_000),
            derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }
    }

    fn updated(received: Vec<Coin>, confirmed: Vec<(bitcoin::OutPoint, i32, u32)>) -> UpdatedCoins {
        UpdatedCoins {
            receive_index: ChildNumber::from_normal_idx(0).unwrap(),
            change_index: ChildNumber::from_normal_idx(0).unwrap(),
            received,
            confirmed,
            expired: Vec::new(),
            spending: Vec::new(),
            expired_spending: Vec::new(),
            spent: Vec::new(),
        }
    }

    fn outpoint(tx: &Transaction) -> bitcoin::OutPoint {
        bitcoin::OutPoint::new(tx.compute_txid(), 0)
    }

    /// A deposit the backend confirms at 100 confirmations is held on BTCB2 until the
    /// long maturity, and confirmed on Bitcoin.
    #[test]
    fn newly_confirmed_coinbase_waits_for_the_chain_maturity() {
        let tx = coinbase_tx(1);
        let op = outpoint(&tx);
        let coinbase: HashSet<_> = std::iter::once(op.txid).collect();
        let curr: HashMap<_, _> = vec![(op, coin(op, None, true))].into_iter().collect();
        let height = 980_000;

        // Bitcoin: the backend's 100 blocks are the rule.
        let mut up = updated(Vec::new(), vec![(op, height, 1)]);
        let marked =
            hold_immature_coinbase(ChainId::Bitcoin, height + 99, &coinbase, &curr, &mut up);
        assert_eq!(up.confirmed, vec![(op, height, 1)]);
        assert!(marked.is_empty());

        // BTCB2 mainnet: 100 confirmations are not enough...
        let mut up = updated(Vec::new(), vec![(op, height, 1)]);
        let marked = hold_immature_coinbase(
            ChainId::BitcoinBlake2b,
            height + 99,
            &coinbase,
            &curr,
            &mut up,
        );
        assert!(up.confirmed.is_empty());
        assert!(marked.is_empty(), "already flagged immature");

        // ...25919 are not either, 25920 are.
        let mut up = updated(Vec::new(), vec![(op, height, 1)]);
        hold_immature_coinbase(
            ChainId::BitcoinBlake2b,
            height + 25_918,
            &coinbase,
            &curr,
            &mut up,
        );
        assert!(up.confirmed.is_empty());
        let mut up = updated(Vec::new(), vec![(op, height, 1)]);
        hold_immature_coinbase(
            ChainId::BitcoinBlake2b,
            height + 25_919,
            &coinbase,
            &curr,
            &mut up,
        );
        assert_eq!(up.confirmed, vec![(op, height, 1)]);
    }

    /// Non-coinbase deposits are never held, whatever the chain.
    #[test]
    fn ordinary_deposits_are_untouched() {
        let tx = coinbase_tx(2);
        let op = outpoint(&tx);
        let curr: HashMap<_, _> = vec![(op, coin(op, None, false))].into_iter().collect();
        let mut up = updated(Vec::new(), vec![(op, 980_000, 1)]);
        let marked = hold_immature_coinbase(
            ChainId::BitcoinBlake2b,
            980_000,
            &HashSet::new(),
            &curr,
            &mut up,
        );
        assert_eq!(up.confirmed.len(), 1);
        assert!(marked.is_empty());
    }

    /// A coinbase deposit first seen during a rescan, already past 100 blocks, comes from
    /// the backend without the immature flag. It is flagged and held.
    #[test]
    fn received_coinbase_is_flagged_from_its_transaction() {
        let tx = coinbase_tx(3);
        let op = outpoint(&tx);
        let coinbase: HashSet<_> = std::iter::once(op.txid).collect();
        let mut up = updated(vec![coin(op, None, false)], vec![(op, 973_500, 1)]);
        hold_immature_coinbase(
            ChainId::BitcoinBlake2bTestnet4,
            973_500 + 200,
            &coinbase,
            &HashMap::new(),
            &mut up,
        );
        assert!(up.confirmed.is_empty());
        assert!(up.received[0].is_immature);
    }

    /// A deposit with a spender was accepted by a node, so it is left as it is.
    #[test]
    fn spent_or_spending_deposits_are_left_alone() {
        let tx = coinbase_tx(4);
        let op = outpoint(&tx);
        let coinbase: HashSet<_> = std::iter::once(op.txid).collect();
        let mut spent = coin(op, Some(980_000), false);
        spent.spend_txid = Some(coinbase_tx(5).compute_txid());
        let curr: HashMap<_, _> = vec![(op, spent)].into_iter().collect();
        let mut up = updated(Vec::new(), Vec::new());
        let marked =
            hold_immature_coinbase(ChainId::BitcoinBlake2b, 980_200, &coinbase, &curr, &mut up);
        assert!(marked.is_empty());

        let curr: HashMap<_, _> = vec![(op, coin(op, None, true))].into_iter().collect();
        let mut up = updated(Vec::new(), vec![(op, 980_000, 1)]);
        up.spending.push((op, coinbase_tx(5).compute_txid()));
        hold_immature_coinbase(ChainId::BitcoinBlake2b, 980_200, &coinbase, &curr, &mut up);
        assert_eq!(up.confirmed.len(), 1);
    }

    /// End to end through a poll: a BTCB2 coinbase deposit an earlier build confirmed at
    /// 100 blocks is put back into the immature state, and one that lost its flag in a
    /// rollback gets it back. A mature one is left confirmed.
    #[test]
    fn poll_re_evaluates_coins_already_marked_mature() {
        let (early, rolled_back, mature) = (coinbase_tx(6), coinbase_tx(7), coinbase_tx(8));
        let mut db = DummyDatabase::new();
        db.set_chain(ChainId::BitcoinBlake2b);
        let mut bit = DummyBitcoind::new();
        let tip_height = 1_000_000;
        bit.tip.height = tip_height;
        db.connection().update_tip(&bit.tip);
        db.connection()
            .new_txs(&[early.clone(), rolled_back.clone(), mature.clone()]);
        db.insert_coins(vec![
            coin(outpoint(&early), Some(tip_height - 150), false),
            coin(outpoint(&rolled_back), None, false),
            coin(outpoint(&mature), Some(tip_height - 25_919), false),
        ]);

        updates(
            &mut db.connection(),
            &mut bit,
            &super::tests::test_descs(),
            &secp256k1::Secp256k1::verification_only(),
            &ReorgAlertCache::default(),
        )
        .unwrap();

        let coins = db.connection().coins(&[], &[]);
        let early = coins[&outpoint(&early)];
        assert!(early.is_immature && early.block_info.is_none());
        let rolled_back = coins[&outpoint(&rolled_back)];
        assert!(rolled_back.is_immature && rolled_back.block_info.is_none());
        let mature = coins[&outpoint(&mature)];
        assert!(!mature.is_immature && mature.block_info.is_some());
    }

    /// The same poll on a Bitcoin wallet changes nothing for a deposit 150 blocks deep.
    #[test]
    fn poll_leaves_bitcoin_coinbase_deposits_confirmed() {
        let tx = coinbase_tx(9);
        let mut db = DummyDatabase::new();
        let mut bit = DummyBitcoind::new();
        bit.tip.height = 1_000;
        db.connection().update_tip(&bit.tip);
        db.connection().new_txs(std::slice::from_ref(&tx));
        db.insert_coins(vec![coin(outpoint(&tx), Some(bit.tip.height - 150), false)]);

        updates(
            &mut db.connection(),
            &mut bit,
            &super::tests::test_descs(),
            &secp256k1::Secp256k1::verification_only(),
            &ReorgAlertCache::default(),
        )
        .unwrap();

        let stored = db.connection().coins(&[], &[])[&outpoint(&tx)];
        assert!(!stored.is_immature && stored.block_info.is_some());
    }
}
