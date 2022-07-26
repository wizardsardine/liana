use crate::bitcoin::BitcoinInterface;

use std::{
    sync::{self, atomic},
    thread, time,
};

/// Main event loop. Repeatedly polls the Bitcoin interface until told to stop through the
/// `shutdown` atomic.
pub fn looper(
    bit: impl BitcoinInterface,
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
                sync_progress
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
    }
}
