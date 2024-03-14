mod looper;

use crate::{bitcoin::BitcoinInterface, database::DatabaseInterface, descriptors};

use std::{
    sync::{self, atomic},
    thread, time,
};

use miniscript::bitcoin::secp256k1;

/// The Bitcoin poller handler.
pub struct Poller {
    bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    // The receive and change descriptors (in this order).
    descs: [descriptors::SinglePathLianaDesc; 2],
}

impl Poller {
    pub fn new(
        bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        desc: descriptors::LianaDescriptor,
    ) -> Poller {
        let secp = secp256k1::Secp256k1::verification_only();
        let descs = [
            desc.receive_descriptor().clone(),
            desc.change_descriptor().clone(),
        ];

        // On first startup the tip may be NULL. Make sure it's set as the poller relies on it.
        looper::maybe_initialize_tip(&bit, &db);

        Poller {
            bit,
            db,
            secp,
            descs,
        }
    }

    /// Continuously update our state from the Bitcoin backend.
    /// - `poll_interval`: how frequently to perform an update.
    /// - `shutdown`: set to true to stop continuously updating and make this function return.
    ///
    /// Typically this would run for the whole duration of the program in a thread, and the main
    /// thread would set the `shutdown` atomic to `true` when shutting down.
    pub fn poll_forever(
        &self,
        poll_interval: time::Duration,
        shutdown: sync::Arc<atomic::AtomicBool>,
    ) {
        let mut last_poll = None;
        let mut synced = false;

        while !shutdown.load(atomic::Ordering::Relaxed) || last_poll.is_none() {
            let now = time::Instant::now();

            if let Some(last_poll) = last_poll {
                let time_since_poll = now.duration_since(last_poll);
                let poll_interval = if synced {
                    poll_interval
                } else {
                    // Until we are synced we poll less often to avoid harassing bitcoind and impeding
                    // the sync. As a function since it's mocked for the tests.
                    looper::sync_poll_interval()
                };
                if time_since_poll < poll_interval {
                    thread::sleep(time::Duration::from_millis(500));
                    continue;
                }
            }
            last_poll = Some(now);

            // Don't poll until the Bitcoin backend is fully synced.
            if !synced {
                let progress = self.bit.sync_progress();
                log::info!(
                    "Block chain synchronization progress: {:.2}% ({} blocks / {} headers)",
                    progress.rounded_up_progress() * 100.0,
                    progress.blocks,
                    progress.headers
                );
                synced = progress.is_complete();
                if !synced {
                    continue;
                }
            }

            looper::poll(&self.bit, &self.db, &self.secp, &self.descs);
        }
    }
}
