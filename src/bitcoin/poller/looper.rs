use crate::{
    bitcoin::{BitcoinInterface, BlockChainTip, UTxO},
    database::{Coin, DatabaseConnection, DatabaseInterface},
    descriptors,
};

use std::{
    sync::{self, atomic},
    thread, time,
};

use miniscript::bitcoin;

#[derive(Debug, Clone)]
struct UpdatedCoins {
    pub received: Vec<Coin>,
    pub confirmed: Vec<(bitcoin::OutPoint, i32, u32)>,
    pub spending: Vec<(bitcoin::OutPoint, bitcoin::Txid)>,
    pub spent: Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)>,
}

// Update the state of our coins. There may be new unspent, and existing ones may become confirmed
// or spent.
// NOTE: A coin may be updated multiple times at once. That is, a coin may be received, confirmed,
// and spent in a single poll.
fn update_coins(
    bit: &impl BitcoinInterface,
    db_conn: &mut Box<dyn DatabaseConnection>,
    previous_tip: &BlockChainTip,
    desc: &descriptors::InheritanceDescriptor,
) -> UpdatedCoins {
    let curr_coins = db_conn.coins();
    log::debug!("Current coins: {:?}", curr_coins);

    // Start by fetching newly received coins.
    let mut received = Vec::new();
    for utxo in bit.received_coins(previous_tip, desc) {
        if let Some(derivation_index) = db_conn.derivation_index_by_address(&utxo.address) {
            if !curr_coins.contains_key(&utxo.outpoint) {
                let UTxO {
                    outpoint, amount, ..
                } = utxo;
                let coin = Coin {
                    outpoint,
                    amount,
                    derivation_index,
                    block_height: None,
                    block_time: None,
                    spend_txid: None,
                    spend_block: None,
                };
                received.push(coin);
            }
        } else {
            log::error!(
                "Could not get derivation index for coin '{}' (address: '{}')",
                &utxo.outpoint,
                &utxo.address
            );
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
            if coin.block_height.is_none() {
                Some(coin.outpoint)
            } else {
                None
            }
        })
        .collect();
    let confirmed = bit.confirmed_coins(&to_be_confirmed);
    log::debug!("Newly confirmed coins: {:?}", confirmed);

    // We need to take the newly received ones into account as well, as they may have been
    // spent within the previous tip and the current one, and we may not poll this chunk of the
    // chain anymore.
    let to_be_spent: Vec<bitcoin::OutPoint> = curr_coins
        .values()
        .chain(received.iter())
        .filter_map(|coin| {
            if coin.spend_txid.is_none() {
                Some(coin.outpoint)
            } else {
                None
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
    let spent = bit.spent_coins(spending_coins.as_slice());
    log::debug!("Newly spent coins: {:?}", spent);

    UpdatedCoins {
        received,
        confirmed,
        spending,
        spent,
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
    let common_ancestor = bit.common_ancestor(current_tip);
    log::info!(
        "Common ancestor found: '{}'. Starting rescan from there. Old tip was '{}'.",
        common_ancestor,
        current_tip
    );
    TipUpdate::Reorged(common_ancestor)
}

fn updates(
    bit: &impl BitcoinInterface,
    db: &impl DatabaseInterface,
    desc: &descriptors::InheritanceDescriptor,
) {
    let mut db_conn = db.connection();

    // Check if there was a new block before updating ourselves.
    let current_tip = db_conn.chain_tip().expect("Always set at first startup");
    let latest_tip = match new_tip(bit, &current_tip) {
        TipUpdate::Same => current_tip,
        TipUpdate::Progress(new_tip) => new_tip,
        TipUpdate::Reorged(new_tip) => {
            // The block chain was reorganized. Rollback our state down to the common ancestor
            // between our former chain and the new one, then restart fresh.
            db_conn.rollback_tip(&new_tip);
            log::info!("Tip was rolled back to '{}'.", new_tip);
            return updates(bit, db, desc);
        }
    };

    // Then check the state of our coins. Do it even if the tip did not change since last poll, as
    // we may have unconfirmed transactions.
    let updated_coins = update_coins(bit, &mut db_conn, &current_tip, desc);

    // If the tip changed while we were polling our Bitcoin interface, start over.
    if bit.chain_tip() != latest_tip {
        log::info!("Chain tip changed while we were updating our state. Starting over.");
        return updates(bit, db, desc);
    }

    // The chain tip did not change since we started our updates. Record them and the latest tip.
    // Having the tip in database means that, as far as the chain is concerned, we've got all
    // updates up to this block. But not more.
    db_conn.new_unspent_coins(&updated_coins.received);
    db_conn.confirm_coins(&updated_coins.confirmed);
    db_conn.spend_coins(&updated_coins.spending);
    db_conn.confirm_spend(&updated_coins.spent);
    if latest_tip != current_tip {
        db_conn.update_tip(&latest_tip);
        log::debug!("New tip: '{}'", latest_tip);
    }

    log::debug!("Updates done.");
}

// If the database chain tip is NULL (first startup), initialize it.
fn maybe_initialize_tip(bit: &impl BitcoinInterface, db: &impl DatabaseInterface) {
    let mut db_conn = db.connection();

    if db_conn.chain_tip().is_none() {
        // TODO: be smarter. We can use the timestamp of the descriptor to get a newer block hash.
        db_conn.update_tip(&bit.genesis_block());
    }
}

/// Main event loop. Repeatedly polls the Bitcoin interface until told to stop through the
/// `shutdown` atomic.
pub fn looper(
    bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    shutdown: sync::Arc<atomic::AtomicBool>,
    poll_interval: time::Duration,
    desc: descriptors::InheritanceDescriptor,
) {
    let mut last_poll = None;
    let mut synced = false;

    maybe_initialize_tip(&bit, &db);

    while !shutdown.load(atomic::Ordering::Relaxed) || last_poll.is_none() {
        let now = time::Instant::now();

        if let Some(last_poll) = last_poll {
            if now.duration_since(last_poll) < poll_interval {
                thread::sleep(time::Duration::from_millis(500));
                continue;
            }
        }
        last_poll = Some(now);

        // Don't poll until the Bitcoin backend is fully synced.
        if !synced {
            let sync_progress = bit.sync_progress();
            log::info!(
                "Block chain synchronization progress: {:.2}%",
                sync_progress * 100.0
            );
            synced = sync_progress == 1.0;
            if !synced {
                // Avoid harassing bitcoind..
                // TODO: be smarter, like in revaultd, but more generic too.
                #[cfg(not(test))]
                thread::sleep(time::Duration::from_secs(30));
                continue;
            }
        }

        updates(&bit, &db, &desc);
    }
}
