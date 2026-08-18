use crate::{
    bitcoin::{
        AncestorSearch, BitcoinInterface, BlockChainTip, ReorgAlertCache, UTxO, UTxOAddress,
    },
    database::{Coin, DatabaseConnection, DatabaseInterface},
};

use std::{collections::HashSet, convert::TryInto, sync, thread, time};

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
    pub received: Vec<Coin>,
    pub confirmed: Vec<(bitcoin::OutPoint, i32, u32)>,
    pub expired: Vec<bitcoin::OutPoint>,
    pub spending: Vec<(bitcoin::OutPoint, bitcoin::Txid)>,
    pub expired_spending: Vec<bitcoin::OutPoint>,
    pub spent: Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)>,
}

// Update the state of our coins. There may be new unspent, and existing ones may become confirmed
// or spent.
// NOTE: A coin may be updated multiple times at once. That is, a coin may be received, confirmed,
// and spent in a single poll.
// NOTE: Coinbase transaction deposits are very much an afterthought here. We treat them as
// unconfirmed until the CB tx matures.
fn update_coins(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    previous_tip: &BlockChainTip,
    descs: &[descriptors::SinglePathCoincubeDesc],
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> UpdatedCoins {
    let network = db_conn.network();
    let curr_coins = db_conn.coins(&[], &[]);
    log::debug!("Current coins: {:?}", curr_coins);

    // Start by fetching newly received coins.
    let mut received = Vec::new();
    for utxo in bit.received_coins(previous_tip, descs) {
        let UTxO {
            outpoint,
            amount,
            address,
            is_immature,
            ..
        } = utxo;
        // We can only really treat them if we know the derivation index that was used.
        let (derivation_index, is_change) = match address {
            UTxOAddress::Address(address) => {
                let address = match address.require_network(network) {
                    Ok(addr) => addr,
                    Err(e) => {
                        log::error!("Invalid network for address: {}", e);
                        continue;
                    }
                };
                if let Some((derivation_index, is_change)) =
                    db_conn.derivation_index_by_address(&address)
                {
                    (derivation_index, is_change)
                } else {
                    // TODO: maybe we could try out something here? Like bruteforcing the next 200 indexes?
                    log::error!(
                        "Could not get derivation index for coin '{}' (address: '{}')",
                        utxo.outpoint,
                        address
                    );
                    continue;
                }
            }
            UTxOAddress::DerivIndex(index, is_change) => (index, is_change),
        };
        // First of if we are receiving coins that are beyond our next derivation index,
        // adjust it.
        if !is_change && derivation_index > db_conn.receive_index() {
            db_conn.set_receive_index(derivation_index, secp);
        } else if is_change && derivation_index > db_conn.change_index() {
            db_conn.set_change_index(derivation_index, secp);
        }

        // Now record this coin as a newly received one.
        if !curr_coins.contains_key(&utxo.outpoint) {
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
    log::debug!("Newly received coins: {:?}", received);

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
    let (confirmed, expired) = bit.confirmed_coins(&to_be_confirmed);
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
    let spending = bit.spending_coins(&to_be_spent);
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
    let (spent, expired_spending) = bit.spent_coins(spending_coins.as_slice());
    log::debug!("Newly spent coins: {:?}", spent);

    UpdatedCoins {
        received,
        confirmed,
        expired,
        spending,
        expired_spending,
        spent,
    }
}

// Add new deposit and spend transactions to the database.
fn add_txs_to_db(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    updated_coins: &UpdatedCoins,
) {
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
        .map(|txid| bit.wallet_transaction(txid).map(|(tx, _)| tx))
        .collect::<Option<Vec<_>>>()
        .expect("we must retrieve all txs");
    if !txs.is_empty() {
        db_conn.new_txs(&txs);
    }
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
        let bitcoin_tip = bit.chain_tip();

        // If the tip didn't change, there is nothing to update.
        if current_tip == &bitcoin_tip {
            return TipUpdate::Same;
        }

        if bitcoin_tip.height > current_tip.height {
            // Make sure we are on the same chain.
            if bit.is_in_chain(current_tip) {
                // All good, we just moved forward.
                return TipUpdate::Progress(bitcoin_tip);
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
            return TipUpdate::Diverged {
                backend_tip: bitcoin_tip,
            };
        }

        log::info!("Block chain reorganization detected. Looking for common ancestor.");
        // A repair we performed on the managed node publishes the block it rewound to.
        // Rollbacks reaching no deeper than that one are ours, not a backend
        // misreporting, so they are both walked for and applied past the limit —
        // otherwise the repaired node and our state stay permanently divorced. See
        // `crate::bitcoin::sanctioned_rollback`.
        //
        // It is addressed to one node, and this backend has to be it. The floor block is
        // public — every node on the chain has it — so without matching the node, an
        // exception raised for the managed node would also disarm the guard for an
        // external `bitcoind` that never underwent the repair.
        let sanctioned = crate::bitcoin::sanctioned_rollback().filter(|sanction| {
            sanction.floor.height < current_tip.height
                && bit.backend_id().as_ref() == Some(&sanction.node)
        });
        // Bounded at one past the limit: any fork we would accept is found inside it, and
        // anything deeper is refused without walking the rest of the way. Unbounded, a
        // backend claiming a 10k-block reorg would cost us 10k sequential round-trips before
        // we rejected the answer. A sanctioned rollback raises the bound just far enough to
        // reach its floor, and no further.
        let max_depth = match &sanctioned {
            Some(sanction) => current_tip
                .height
                .saturating_sub(sanction.floor.height)
                .max(MAX_REORG_DEPTH),
            None => MAX_REORG_DEPTH,
        };
        match bit.common_ancestor(current_tip, max_depth + 1) {
            AncestorSearch::Found(common_ancestor) => {
                let depth = current_tip.height.saturating_sub(common_ancestor.height);
                if depth > MAX_REORG_DEPTH {
                    // Having already established this is the node the repair was
                    // performed on: the fork must be at or above the floor it rewound
                    // to, and that floor block must still be in the node's chain — a
                    // sanction surviving a datadir replaced under the same RPC port
                    // authorises nothing.
                    let authorised = sanctioned.as_ref().is_some_and(|sanction| {
                        common_ancestor.height >= sanction.floor.height
                            && bit.is_in_chain(&sanction.floor)
                    });
                    if !authorised {
                        return TipUpdate::ImplausibleReorg { min_depth: depth };
                    }
                    log::warn!(
                        "Applying a {}-block rollback to '{}', past the {}-block limit: it lands \
                         on '{}', at or above the block a managed-node repair rewound this chain \
                         to.",
                        depth,
                        current_tip,
                        MAX_REORG_DEPTH,
                        common_ancestor
                    );
                }
                log::info!(
                    "Common ancestor found: '{}'. Starting rescan from there. Old tip was '{}'.",
                    common_ancestor,
                    current_tip
                );
                return TipUpdate::Reorged(common_ancestor);
            }
            // An answer, not a failure: retrying would just spend the same round-trips again
            // to reach the same conclusion.
            // Deeper than we were willing to walk — including past a sanctioned rollback,
            // which means the fork is not the one we authorised.
            AncestorSearch::TooDeep => {
                return TipUpdate::ImplausibleReorg {
                    min_depth: max_depth + 1,
                }
            }
            AncestorSearch::Failed => {}
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

/// Whether a refused reorg's depth was measured or merely bounded.
#[derive(Debug, Clone, Copy)]
enum RefusedDepth {
    /// We reached the fork point, so this is the true depth.
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

fn updates(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut impl BitcoinInterface,
    descs: &[descriptors::SinglePathCoincubeDesc],
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    reorg_alert: &ReorgAlertCache,
) {
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
                    return updates(db_conn, bit, descs, secp, reorg_alert);
                }
                TipUpdate::ImplausibleReorg { min_depth } => {
                    refuse_deep_reorg(reorg_alert, &current_tip, RefusedDepth::AtLeast(min_depth));
                    return;
                }
                TipUpdate::Diverged { backend_tip } => {
                    report_divergence(reorg_alert, &current_tip, &backend_tip);
                    return;
                }
                TipUpdate::Unavailable => return,
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
                // common ancestor directly (Electrum) can misreport just as badly as those we
                // walk back ourselves, and the rollback is equally destructive either way.
                let depth = current_tip
                    .height
                    .saturating_sub(reorg_common_ancestor.height);
                if depth > MAX_REORG_DEPTH {
                    refuse_deep_reorg(reorg_alert, &current_tip, RefusedDepth::Exact(depth));
                    return;
                }
                db_conn.rollback_tip(&reorg_common_ancestor);
                log::info!("Tip was rolled back to '{}'.", reorg_common_ancestor);
            } else {
                log::info!(
                    "Tip was already earlier than common ancestor '{}'.",
                    reorg_common_ancestor
                );
            }
            return updates(db_conn, bit, descs, secp, reorg_alert);
        }
        Err(e) => {
            // The Esplora "every provider cooling" case isn't a real
            // failure — no network call was attempted — so log it at
            // debug and back off long enough for at least one
            // provider's cooldown to expire (cooldowns run 10 min,
            // so 30s here just keeps the tick from being wasteful;
            // future ticks will still skip-fast until something
            // recovers). Without this branch the poller spammed
            // ~30 ERROR lines/min while every backend was throttled.
            // The string check is intentional: the
            // `BitcoinInterface::sync_wallet` signature returns
            // `Result<_, String>`, so we can't pattern-match on the
            // typed `client::Error::AllCooling` variant from here.
            // A regression test in `bitcoin::esplora::client`
            // guards the marker.
            // A scan aborted by `DaemonHandle::stop` must NOT retry: return so
            // `poll_forever` regains control and processes the Shutdown message.
            // Recursing the 2s retry below would re-issue the (now instantly
            // aborting) scan forever and never let the poller exit — leaving
            // `stop()` blocked on the join.
            if e.contains(crate::bitcoin::esplora::client::SCAN_ABORTED_DISPLAY_MARKER) {
                log::debug!("Esplora poll aborted — daemon shutting down");
                return;
            }
            if e.contains(crate::bitcoin::esplora::client::ALL_COOLING_DISPLAY_MARKER) {
                log::debug!("Esplora poll skipped: {}", e);
                thread::sleep(time::Duration::from_secs(30));
            } else {
                log::error!("Error syncing wallet: '{}'.", e);
                thread::sleep(time::Duration::from_secs(2));
            }
            return updates(db_conn, bit, descs, secp, reorg_alert);
        }
    };

    // We got a coherent answer out of the backend and its chain contains our tip, so any
    // outstanding chain alert — a refused deep reorg or an unresolved divergence — no
    // longer reflects reality: synchronization has resumed. Clear it before touching coins.
    reorg_alert.clear();

    // Then check the state of our coins. Do it even if the tip did not change since last poll, as
    // we may have unconfirmed transactions.
    let updated_coins = update_coins(bit, db_conn, &current_tip, descs, secp);

    // If the tip changed while we were polling our Bitcoin interface, start over.
    if bit.chain_tip() != latest_tip {
        log::info!("Chain tip changed while we were updating our state. Starting over.");
        return updates(db_conn, bit, descs, secp, reorg_alert);
    }

    // Transactions must be added to the DB before coins due to foreign key constraints.
    add_txs_to_db(bit, db_conn, &updated_coins);
    // The chain tip did not change since we started our updates. Record them and the latest tip.
    // Having the tip in database means that, as far as the chain is concerned, we've got all
    // updates up to this block. But not more.
    db_conn.new_unspent_coins(&updated_coins.received);
    db_conn.remove_coins(&updated_coins.expired);
    db_conn.confirm_coins(&updated_coins.confirmed);
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
}

// Check if there is any rescan of the backend ongoing or one that just finished.
fn rescan_check(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut impl BitcoinInterface,
    descs: &[descriptors::SinglePathCoincubeDesc],
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    reorg_alert: &ReorgAlertCache,
) {
    log::debug!("Checking the state of an ongoing rescan if there is any");

    // Check if there is an ongoing rescan. If there isn't and we previously asked for a rescan of
    // the backend, we treat it as completed.
    // Upon completion of the rescan from the given timestamp on the backend, we rollback our state
    // down to the height before this timestamp to rescan everything that happened since then.
    let rescan_timestamp = db_conn.rescan_timestamp();
    if let Some(progress) = bit.rescan_progress() {
        log::info!("Rescan progress: {:.2}%.", progress * 100.0);
        if rescan_timestamp.is_none() {
            log::warn!("Backend is rescanning but we didn't ask for it.");
        }
    } else if let Some(timestamp) = rescan_timestamp {
        log::info!("Rescan completed on the backend.");
        // TODO: we could check if the timestamp of the descriptors in the Bitcoin backend are
        // truly at the rescan timestamp, and trigger a rescan otherwise. Note however it would be
        // no use for the bitcoind implementation of the backend, since bitcoind will always set
        // the timestamp of the descriptors in the wallet first (and therefore consider it as
        // rescanned from this height even if it aborts the rescan by being stopped).
        let rescan_tip = match bit.block_before_date(timestamp) {
            Some(block) => block,
            None => {
                log::error!(
                    "Could not retrieve block height for timestamp '{}'",
                    timestamp
                );
                return;
            }
        };
        db_conn.rollback_tip(&rescan_tip);
        db_conn.complete_rescan();
        log::info!(
            "Rolling back our internal tip to '{}' to update our internal state with past transactions.",
            rescan_tip
        );
        updates(db_conn, bit, descs, secp, reorg_alert)
    } else {
        log::debug!("No ongoing rescan.");
    }
}

/// If the database chain tip is NULL (first startup), initialize it.
pub fn maybe_initialize_tip(bit: &impl BitcoinInterface, db: &impl DatabaseInterface) {
    let mut db_conn = db.connection();

    if db_conn.chain_tip().is_none() {
        // TODO: be smarter. We can use the timestamp of the descriptor to get a newer block hash.
        db_conn.update_tip(&bit.genesis_block());
    }
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
    descs: &[descriptors::SinglePathCoincubeDesc],
    reorg_alert: &ReorgAlertCache,
) {
    let mut db_conn = db.connection();
    updates(&mut db_conn, bit, descs, secp, reorg_alert);
    rescan_check(&mut db_conn, bit, descs, secp, reorg_alert);
    let now: u32 = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .expect("current system time must be later than epoch")
        .as_secs()
        .try_into()
        .expect("system clock year is earlier than 2106");
    db_conn.set_last_poll(now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bitcoin::ChainAlert,
        database::{Coin, DatabaseInterface},
        testutils::{DummyBitcoind, DummyDatabase},
    };
    use std::str::FromStr;

    use miniscript::{
        bitcoin::{bip32, hashes::Hash},
        descriptor,
    };

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

    /// Serialises the tests that read or write the process-wide sanctioned-rollback
    /// slot. Without it a test that arms the slot can change what a concurrently
    /// running depth-guard test observes.
    static SANCTION_LOCK: sync::Mutex<()> = sync::Mutex::new(());

    /// Arms the sanctioned-rollback slot and disarms it again on drop, so a failing
    /// assertion cannot leak the exception into the rest of the suite.
    struct Sanction(#[allow(dead_code)] sync::MutexGuard<'static, ()>);

    impl Sanction {
        /// Armed for the node `DummyBitcoind` reports itself as.
        fn arm(floor: BlockChainTip) -> Self {
            Self::arm_for(
                floor,
                crate::testutils::DUMMY_RPC_ADDR,
                crate::testutils::DUMMY_CREDENTIALS,
            )
        }

        fn arm_for(floor: BlockChainTip, addr: &str, credentials: &str) -> Self {
            let guard = SANCTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            crate::bitcoin::set_sanctioned_rollback(Some(crate::bitcoin::SanctionedRollback {
                floor,
                node: crate::testutils::dummy_backend_id(addr, credentials),
            }));
            Self(guard)
        }

        fn none() -> Self {
            let guard = SANCTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            crate::bitcoin::set_sanctioned_rollback(None);
            Self(guard)
        }
    }

    impl Drop for Sanction {
        fn drop(&mut self) {
            crate::bitcoin::set_sanctioned_rollback(None);
        }
    }

    /// A backend that forks `depth` below our tip and whose chain still contains
    /// `floor` — the shape a repaired managed node has: rewound to the floor, then
    /// reconnected back up to wherever it stopped accepting blocks.
    fn repaired_backend(
        our_tip: &BlockChainTip,
        depth: i32,
        floor: BlockChainTip,
    ) -> DummyBitcoind {
        let mut bit = forked_backend(our_tip, depth);
        bit.also_in_chain = vec![floor];
        bit
    }

    // A repair we performed ourselves rewinds the managed node far past the depth
    // limit. Refusing it is what left every Vault pinned to a chain the node no
    // longer had: maintenance ends, and every later poll re-refuses the same reorg.
    #[test]
    fn a_sanctioned_rollback_is_applied_past_the_limit() {
        let our_tip = tip(20_000, 0xaa);
        // Rewound to 10,000 below our tip. Far deeper than the walk's usual bound, so
        // this also covers the case where the fork point could not even be reached.
        let floor = tip(10_000, 0xcc);
        let bit = repaired_backend(&our_tip, 10_000, floor);

        let _armed = Sanction::arm(floor);
        match new_tip(&bit, &our_tip) {
            TipUpdate::Reorged(ancestor) => assert_eq!(ancestor, floor),
            other => panic!(
                "expected the sanctioned rollback to be applied, got {:?}",
                other
            ),
        }
    }

    // The realistic outcome, and the one an exact-block exception got wrong: the node
    // replays most of what it rewound and only refuses a block near the top, so the
    // chains part company thousands of blocks *above* the floor. That fork point is
    // not knowable when the repair starts, which is why the sanction is a floor.
    #[test]
    fn a_rollback_above_the_sanctioned_floor_is_applied() {
        let our_tip = tip(20_000, 0xaa);
        let floor = tip(10_000, 0xcc);
        // Rewound to 10,000; everything up to 19,000 was re-accepted, 19,001 was not.
        let bit = repaired_backend(&our_tip, 1_000, floor);

        let _armed = Sanction::arm(floor);
        match new_tip(&bit, &our_tip) {
            TipUpdate::Reorged(ancestor) => assert_eq!(ancestor.height, 19_000),
            other => panic!(
                "expected a fork above the sanctioned floor to be applied, got {:?}",
                other
            ),
        }
    }

    // The floor is a limit, not a licence. Anything deeper than it, and any backend
    // whose chain does not contain the floor block at all, is refused as before.
    #[test]
    fn a_sanction_authorises_nothing_below_its_floor_or_off_its_chain() {
        let our_tip = tip(20_000, 0xaa);
        let floor = tip(10_000, 0xcc);

        // A fork 1,000 blocks below the floor: outside what the repair could produce.
        let bit = repaired_backend(&our_tip, 11_000, floor);
        let _armed = Sanction::arm(floor);
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { .. } => {}
            other => panic!(
                "expected a fork below the floor to be refused, got {:?}",
                other
            ),
        }
        drop(_armed);

        // Right depth, but this backend's chain never contained the block we rewound
        // to — a datadir replaced under the same RPC port.
        let mut bit = forked_backend(&our_tip, 10_000);
        bit.also_in_chain = vec![tip(10_000, 0xee)];
        let _armed = Sanction::arm(floor);
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { .. } => {}
            other => panic!(
                "expected a sanction from another chain to authorise nothing, got {:?}",
                other
            ),
        }
    }

    // The floor block is public: every node on the chain has it, so a chain-level
    // check cannot tell the repaired node from any other. Without the endpoint in the
    // exception, a Vault pointed at an external `bitcoind` that never underwent the
    // repair would lose its depth guard for everything above the floor.
    #[test]
    fn a_sanction_authorises_nothing_on_another_node() {
        let our_tip = tip(20_000, 0xaa);
        let floor = tip(10_000, 0xcc);
        // Same chain, same floor block, different node.
        let bit = repaired_backend(&our_tip, 10_000, floor);

        let _armed = Sanction::arm_for(
            floor,
            "127.0.0.1:18332",
            crate::testutils::DUMMY_CREDENTIALS,
        );
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { .. } => {}
            other => panic!(
                "expected a sanction for another node to authorise nothing, got {:?}",
                other
            ),
        }
        drop(_armed);

        // Same address, different credentials — the shape a datadir replaced under
        // the same port takes, since the cookie file lives inside the datadir.
        let bit = repaired_backend(&our_tip, 10_000, floor);
        let _armed = Sanction::arm_for(
            floor,
            crate::testutils::DUMMY_RPC_ADDR,
            "cookie:/somewhere/else/.cookie",
        );
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { .. } => {}
            other => panic!(
                "expected a sanction for another datadir to authorise nothing, got {:?}",
                other
            ),
        }
        drop(_armed);

        // And a backend that is not a bitcoind at all has no identity to match.
        let mut bit = repaired_backend(&our_tip, 10_000, floor);
        bit.backend_id = None;
        let _armed = Sanction::arm(floor);
        match new_tip(&bit, &our_tip) {
            TipUpdate::ImplausibleReorg { .. } => {}
            other => panic!(
                "expected an unidentifiable backend to authorise nothing, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn deep_reorg_is_refused() {
        let _disarmed = Sanction::none();
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

        // And so is the 10k-block rewind an `invalidateblock` at the RDTS anchor
        // would produce. Here the walk stops at its bound rather than paying for
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
        let _disarmed = Sanction::none();
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

    fn test_descs() -> [descriptors::SinglePathCoincubeDesc; 2] {
        let owner_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap());
        let heir_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap());
        let policy = descriptors::CoincubePolicy::new_legacy(
            owner_key,
            [(10u16, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);
        [
            desc.receive_descriptor().clone(),
            desc.change_descriptor().clone(),
        ]
    }

    /// The acceptance test for this guard: a 10k-block rewind — what an `invalidateblock`
    /// at the RDTS anchor looks like to the poller — must not roll our tip back and must
    /// not delete a single coin row.
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

        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

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
        let _disarmed = Sanction::none();
        let (db, outpoint) = wallet_with_one_coin(&our_tip);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

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
        let _disarmed = Sanction::none();
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
        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

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
        let _disarmed = Sanction::none();
        let our_tip = tip(146_244, 0xaa);
        let (db, outpoint) = wallet_with_one_coin(&our_tip);
        let descs = test_descs();
        let secp = secp256k1::Secp256k1::verification_only();
        let alert = ReorgAlertCache::default();
        let mut db_conn = db.connection();

        // First poll: diverged. The status is published and wallet state is held.
        let mut bit = diverged_backend(&our_tip, our_tip.height + 5);
        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);
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
        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

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
        updates(&mut db_conn, &mut bit, &descs, &secp, &alert);

        assert_eq!(db.rollbacks(), vec![tip(20_000 - 3, 0xcc)]);
        assert_eq!(
            alert.load(),
            ChainAlert::None,
            "an applied reorg raises no alert"
        );
    }
}
