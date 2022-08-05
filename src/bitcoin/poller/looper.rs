use crate::{
    bitcoin::BitcoinInterface,
    database::{DatabaseConnection, DatabaseInterface},
};

use std::{
    sync::{self, atomic},
    thread, time,
};

fn update_tip(bit: &impl BitcoinInterface, db_conn: &mut Box<dyn DatabaseConnection>) {
    let bitcoin_tip = bit.chain_tip();

    let current_tip = match db_conn.chain_tip() {
        Some(tip) => tip,
        None => {
            db_conn.update_tip(&bitcoin_tip);
            return;
        }
    };

    // If the tip didn't change, there is nothing to update.
    if current_tip == bitcoin_tip {
        return;
    }

    if bitcoin_tip.height > current_tip.height {
        // Make sure we are on the same chain.
        if bit.is_in_chain(&current_tip) {
            // All good, we just moved forward. Record the new tip.
            db_conn.update_tip(&bitcoin_tip);
            return;
        }
    }

    // TODO: reorg handling.
}

/// Main event loop. Repeatedly polls the Bitcoin interface until told to stop through the
/// `shutdown` atomic.
pub fn looper(
    bit: impl BitcoinInterface,
    db: impl DatabaseInterface,
    shutdown: sync::Arc<atomic::AtomicBool>,
    poll_interval: time::Duration,
) {
    let mut last_poll = None;
    let mut synced = false;

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

        let mut db_conn = db.connection();
        update_tip(&bit, &mut db_conn);
    }
}
