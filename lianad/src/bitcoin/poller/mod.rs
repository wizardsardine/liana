mod looper;

use crate::{bitcoin::BitcoinInterface, database::DatabaseInterface};
use liana::descriptors;

use std::{
    sync::{self, mpsc},
    time,
};

use miniscript::bitcoin::secp256k1;

#[derive(Debug, Clone)]
pub enum PollerMessage {
    Shutdown,
    /// Ask the Bitcoin poller to poll immediately, get notified through the passed channel once
    /// it's done.
    PollNow(mpsc::SyncSender<()>),
}

/// The Bitcoin poller handler.
pub struct Poller {
    bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    desc: descriptors::LianaDescriptor,
}

impl Poller {
    pub fn new(
        bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        desc: descriptors::LianaDescriptor,
    ) -> Poller {
        let secp = secp256k1::Secp256k1::verification_only();

        // On first startup the tip may be NULL. Make sure it's set as the poller relies on it.
        looper::maybe_initialize_tip(&bit, &db);

        Poller {
            bit,
            db,
            secp,
            desc,
        }
    }

    /// Continuously update our state from the Bitcoin backend.
    /// - `poll_interval`: how frequently to perform an update.
    /// - `shutdown`: set to true to stop continuously updating and make this function return.
    ///
    /// Typically this would run for the whole duration of the program in a thread, and the main
    /// thread would set the `shutdown` atomic to `true` when shutting down.
    pub fn poll_forever(
        &mut self,
        poll_interval: time::Duration,
        receiver: mpsc::Receiver<PollerMessage>,
    ) {
        let mut last_poll = None;
        let mut synced = false;

        loop {
            // How long to wait before the next poll.
            let time_before_poll = if let Some(last_poll) = last_poll {
                let time_since_poll = time::Instant::now().duration_since(last_poll);
                // Until we are synced we poll less often to avoid harassing bitcoind and impeding
                // the sync. As a function since it's mocked for the tests.
                let poll_interval = if synced {
                    poll_interval
                } else {
                    looper::sync_poll_interval()
                };
                poll_interval.saturating_sub(time_since_poll)
            } else {
                // Don't wait before doing the first poll.
                time::Duration::ZERO
            };

            // Wait for the duration of the interval between polls, but listen to messages in the
            // meantime.
            match receiver.recv_timeout(time_before_poll) {
                Ok(PollerMessage::Shutdown) => {
                    log::info!("Bitcoin poller was told to shut down.");
                    return;
                }
                Ok(PollerMessage::PollNow(sender)) => {
                    // We've been asked to poll, don't wait any further and signal completion to
                    // the caller, unless the block chain is still syncing.
                    // Polling while the block chain is syncing could lead to poller restarts
                    // if the height increases before completion, and in any case this is consistent
                    // with regular poller behaviour.
                    if !synced {
                        let progress = self.bit.sync_progress();
                        log::info!(
                            "Block chain synchronization progress: {:.2}% ({} blocks / {} headers)",
                            progress.rounded_up_progress() * 100.0,
                            progress.blocks,
                            progress.headers
                        );
                        synced = progress.is_complete();
                    }
                    // Update `last_poll` even if we don't poll now so that we don't attempt another
                    // poll too soon.
                    last_poll = Some(time::Instant::now());
                    if synced {
                        looper::poll(&mut self.bit, &self.db, &self.secp, &self.desc);
                    } else {
                        log::warn!("Skipped poll as block chain is still synchronizing.");
                    }
                    if let Err(e) = sender.send(()) {
                        log::error!("Error sending immediate poll completion signal: {}.", e);
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // It's been long enough since the last poll.
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::error!("Bitcoin poller communication channel got disconnected. Exiting.");
                    return;
                }
            }
            last_poll = Some(time::Instant::now());

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

            looper::poll(&mut self.bit, &self.db, &self.secp, &self.desc);
        }
    }
}
