use crate::{
    bitcoin::{BitcoinInterface, BlockChainTip, UTxO, UTxOAddress},
    database::{Coin, DatabaseConnection, DatabaseInterface},
    payjoin::{receiver::payjoin_receiver_check, sender::payjoin_sender_check},
};

use std::{collections::HashSet, convert::TryInto, sync, thread, time};

use liana::descriptors;
use miniscript::bitcoin::{self, secp256k1};

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
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> UpdatedCoins {
    let network = db_conn.network();
    let curr_coins = db_conn.coins(&[], &[]);
    log::debug!("Current coins: {:?}", curr_coins);

    // Start by fetching newly received coins.
    let descs = &[
        desc.receive_descriptor().clone(),
        desc.change_descriptor().clone(),
    ];
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
                        &utxo.outpoint,
                        &address
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
}

// Returns the new block chain tip, if it changed.
fn new_tip(bit: &impl BitcoinInterface, current_tip: &BlockChainTip) -> TipUpdate {
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
    log::info!("Block chain reorganization detected. Looking for common ancestor.");
    if let Some(common_ancestor) = bit.common_ancestor(current_tip) {
        log::info!(
            "Common ancestor found: '{}'. Starting rescan from there. Old tip was '{}'.",
            common_ancestor,
            current_tip
        );
        TipUpdate::Reorged(common_ancestor)
    } else {
        log::error!(
            "Failed to get common ancestor for tip '{}'. Starting over.",
            current_tip
        );
        new_tip(bit, current_tip)
    }
}

fn updates(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut impl BitcoinInterface,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
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
                    return updates(db_conn, bit, desc, secp);
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
                db_conn.rollback_tip(&reorg_common_ancestor);
                log::info!("Tip was rolled back to '{}'.", &reorg_common_ancestor);
            } else {
                log::info!(
                    "Tip was already earlier than common ancestor '{}'.",
                    &reorg_common_ancestor
                );
            }
            return updates(db_conn, bit, desc, secp);
        }
        Err(e) => {
            log::error!("Error syncing wallet: '{}'.", e);
            thread::sleep(time::Duration::from_secs(2));
            return updates(db_conn, bit, desc, secp);
        }
    };

    // Then check the state of our coins. Do it even if the tip did not change since last poll, as
    // we may have unconfirmed transactions.
    let updated_coins = update_coins(bit, db_conn, &current_tip, desc, secp);

    // If the tip changed while we were polling our Bitcoin interface, start over.
    if bit.chain_tip() != latest_tip {
        log::info!("Chain tip changed while we were updating our state. Starting over.");
        return updates(db_conn, bit, desc, secp);
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
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
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
        updates(db_conn, bit, desc, secp)
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
    desc: &descriptors::LianaDescriptor,
) {
    let mut db_conn = db.connection();
    updates(&mut db_conn, bit, desc, secp);
    rescan_check(&mut db_conn, bit, desc, secp);
    payjoin_sender_check(db);
    payjoin_receiver_check(db, bit, desc, secp);
    let now: u32 = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .expect("current system time must be later than epoch")
        .as_secs()
        .try_into()
        .expect("system clock year is earlier than 2106");
    db_conn.set_last_poll(now);
}
