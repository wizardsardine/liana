mod looper;

use crate::{bitcoin::BitcoinInterface, database::DatabaseInterface};
use coincube_core::descriptors;

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
    /// Same as [`Self::PollNow`] but without an ack channel.
    /// Intended for fire-and-forget triggers (UX hooks: app focus,
    /// Receive panel open, new receive address) where the caller
    /// has no use for the completion signal — and where supplying
    /// a sender whose matching receiver gets dropped immediately
    /// would cause the poller to log a spurious "send failed"
    /// error on every trigger.
    PollNowNoAck,
}

/// The Bitcoin poller handler.
pub struct Poller {
    bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    // The receive and change descriptors (in this order).
    descs: [descriptors::SinglePathCoincubeDesc; 2],
    // Lock-free mirror of the latest sync progress, so `get_info` can read it
    // without contending for the `BitcoinInterface` mutex this poller holds
    // across a full wallet scan.
    sync_cache: sync::Arc<crate::bitcoin::SyncProgressCache>,
    // Published when a poll declines an implausibly deep reorg, so `get_info` can
    // report it without taking the `BitcoinInterface` mutex.
    reorg_alert: sync::Arc<crate::bitcoin::ReorgAlertCache>,
}

impl Poller {
    pub fn new(
        bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        desc: descriptors::CoincubeDescriptor,
        sync_cache: sync::Arc<crate::bitcoin::SyncProgressCache>,
        reorg_alert: sync::Arc<crate::bitcoin::ReorgAlertCache>,
    ) -> Poller {
        let secp = secp256k1::Secp256k1::verification_only();
        let descs = [
            desc.receive_descriptor().clone(),
            desc.change_descriptor().clone(),
        ];

        // On first startup the tip may be NULL. Make sure it's set as the poller relies on it.
        looper::maybe_initialize_tip(&bit, &db);

        // NB: we deliberately do NOT read `sync_progress` here to seed the
        // cache. That would add a backend RPC during construction (which the
        // scripted `daemon_startup` test doesn't expect, and which would
        // re-introduce a startup round-trip). The cache's zero default reads as
        // "still syncing", and the poll loop — whose first tick has no initial
        // delay — publishes the real value immediately.

        Poller {
            bit,
            db,
            secp,
            descs,
            sync_cache,
            reorg_alert,
        }
    }

    /// Shared body for the immediate-poll message arms
    /// ([`PollerMessage::PollNow`] and [`PollerMessage::PollNowNoAck`]).
    /// Updates `synced` and `last_poll` regardless of whether the
    /// chain is actually caught up, so the caller doesn't have to
    /// repeat that bookkeeping or decide independently when the
    /// next scheduled tick should fire.
    fn run_immediate_poll(&mut self, synced: &mut bool, last_poll: &mut Option<time::Instant>) {
        // An explicit "poll now" is still refused during a rewind — the caller wants
        // fresh state, and mid-rewind the node has none to give.
        if self.paused_for_node_maintenance() {
            log::debug!("Skipped immediate poll: the managed node is under maintenance.");
            *last_poll = Some(time::Instant::now());
            return;
        }
        // Polling while the block chain is syncing could lead to
        // poller restarts if the height increases before
        // completion, and in any case this is consistent with
        // regular poller behaviour: re-check sync progress before
        // committing to a poll.
        if !*synced {
            let progress = self.bit.sync_progress();
            self.sync_cache.store(&progress);
            log::info!(
                "Block chain synchronization progress: {:.2}% ({} blocks / {} headers)",
                progress.rounded_up_progress() * 100.0,
                progress.blocks,
                progress.headers
            );
            *synced = progress.is_complete();
        }
        // Update `last_poll` even if we don't poll now so that we
        // don't attempt another poll too soon.
        *last_poll = Some(time::Instant::now());
        if *synced {
            looper::poll(
                &mut self.bit,
                &self.db,
                &self.secp,
                &self.descs,
                &self.reorg_alert,
            );
        } else {
            log::warn!("Skipped poll as block chain is still synchronizing.");
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
                    self.run_immediate_poll(&mut synced, &mut last_poll);
                    if let Err(e) = sender.send(()) {
                        log::error!("Error sending immediate poll completion signal: {}.", e);
                    }
                    continue;
                }
                Ok(PollerMessage::PollNowNoAck) => {
                    // Same work, no completion signal — see the
                    // `PollNowNoAck` variant doc for why this
                    // exists separately from `PollNow`.
                    self.run_immediate_poll(&mut synced, &mut last_poll);
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

            // Stand down while the managed node is being deliberately rewound: the
            // chain it would show us mid-operation is not one to record.
            if self.paused_for_node_maintenance() {
                log::debug!("Skipped poll: the managed node is under maintenance.");
                continue;
            }

            // Don't poll until the Bitcoin backend is fully synced.
            if !synced {
                let progress = self.bit.sync_progress();
                self.sync_cache.store(&progress);
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

            looper::poll(
                &mut self.bit,
                &self.db,
                &self.secp,
                &self.descs,
                &self.reorg_alert,
            );
        }
    }

    /// Whether to skip this tick because the shared managed node is being rewound.
    ///
    /// Only bitcoind-backed Vaults stand down: an Esplora or Electrum Vault has its
    /// own view of the chain and is unaffected by whatever we do to the managed node.
    fn paused_for_node_maintenance(&self) -> bool {
        crate::bitcoin::managed_node_maintenance() && self.bit.is_bitcoind()
    }
}
