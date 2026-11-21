mod looper;
#[cfg(test)]
pub(crate) use looper::poll as test_poll;

use crate::{bitcoin::BitcoinInterface, database::DatabaseInterface};
use coincube_core::wallet_descriptor::WalletDescriptor;

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
    descs: [miniscript::Descriptor<miniscript::DescriptorPublicKey>; 2],
    // Lock-free mirror of the latest sync progress, so `get_info` can read it
    // without contending for the `BitcoinInterface` mutex this poller holds
    // across a full wallet scan.
    sync_cache: sync::Arc<crate::bitcoin::SyncProgressCache>,
    // Published when a poll declines an implausibly deep reorg, so `get_info` can
    // report it without taking the `BitcoinInterface` mutex.
    reorg_alert: sync::Arc<crate::bitcoin::ReorgAlertCache>,
    // Where each poll's outcome is recorded, so `get_info` can tell a wallet whose
    // history is still loading, or failing to load, from one that is up to date.
    history_sync: sync::Arc<crate::bitcoin::HistorySyncCache>,
    // The tip and its header time, so a new spend's locktime is set without
    // waiting for the `BitcoinInterface` mutex this poller holds across a sync.
    tip_cache: sync::Arc<crate::bitcoin::TipTimeCache>,
}

impl Poller {
    pub fn new(
        bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        desc: WalletDescriptor,
        sync_cache: sync::Arc<crate::bitcoin::SyncProgressCache>,
        reorg_alert: sync::Arc<crate::bitcoin::ReorgAlertCache>,
        history_sync: sync::Arc<crate::bitcoin::HistorySyncCache>,
        tip_cache: sync::Arc<crate::bitcoin::TipTimeCache>,
    ) -> Result<Poller, crate::bitcoin::GenesisError> {
        let secp = secp256k1::Secp256k1::verification_only();
        let (receive, change) = desc.branches_public();
        let descs = [receive.clone(), change.clone()];

        // On first startup the tip may be NULL. Make sure it's set as the poller relies on it.
        looper::maybe_initialize_tip(&bit, &db)?;

        // NB: we deliberately do NOT read `sync_progress` here to seed the
        // cache. That would add a backend RPC during construction (which the
        // scripted `daemon_startup` test doesn't expect, and which would
        // re-introduce a startup round-trip). The cache's zero default reads as
        // "still syncing", and the poll loop — whose first tick has no initial
        // delay — publishes the real value immediately.

        Ok(Poller {
            bit,
            db,
            secp,
            descs,
            sync_cache,
            reorg_alert,
            history_sync,
            tip_cache,
        })
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
            let progress = match self.bit.try_sync_progress() {
                Ok(progress) => progress,
                Err(error) => {
                    log::debug!("Sync progress unavailable: {}", error);
                    *last_poll = Some(time::Instant::now());
                    return;
                }
            };
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
            self.poll_once();
        } else {
            log::warn!("Skipped poll as block chain is still synchronizing.");
        }
    }

    /// One poll, with the tip published to `tip_cache` before it (so a spend
    /// made during the first, possibly long, sync has a tip) and after it.
    fn poll_once(&mut self) {
        self.publish_tip();
        self.apply_eager_request();
        looper::poll(
            &mut self.bit,
            &self.db,
            &self.secp,
            &self.descs,
            &self.reorg_alert,
            &self.history_sync,
        );
        self.publish_tip();
    }

    /// Record the backend's tip and its header time.
    fn publish_tip(&mut self) {
        publish_tip(&self.bit, &self.tip_cache);
    }

    fn apply_eager_request(&mut self) {
        if self.history_sync.take_eager_sync_request() {
            self.bit.lock().unwrap().request_eager_sync();
        }
    }

    fn time_before_poll(
        last_poll: Option<time::Instant>,
        synced: bool,
        poll_interval: time::Duration,
    ) -> time::Duration {
        if let Some(last_poll) = last_poll {
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
            let time_before_poll = Self::time_before_poll(last_poll, synced, poll_interval);

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
            if !synced && !self.scheduled_backend_ready(&mut synced) {
                continue;
            }

            self.poll_once();
        }
    }

    fn scheduled_backend_ready(&mut self, synced: &mut bool) -> bool {
        let progress = match self.bit.try_sync_progress() {
            Ok(progress) => progress,
            Err(error) => {
                log::debug!("Sync progress unavailable: {}", error);
                return false;
            }
        };
        self.sync_cache.store(&progress);
        log::info!(
            "Block chain synchronization progress: {:.2}% ({} blocks / {} headers)",
            progress.rounded_up_progress() * 100.0,
            progress.blocks,
            progress.headers
        );
        *synced = progress.is_complete();
        *synced
    }

    /// Whether to skip this tick because the shared managed node is being rewound.
    ///
    /// Only bitcoind-backed Vaults stand down: an Esplora or Electrum Vault has its
    /// own view of the chain and is unaffected by whatever we do to the managed node.
    fn paused_for_node_maintenance(&self) -> bool {
        crate::bitcoin::managed_node_maintenance() && self.bit.is_bitcoind()
    }
}

/// Publish the backend's tip and its header time to `cache`. The time is read
/// only when the tip has changed: over Esplora it is an HTTP request.
///
/// A cached tip above the backend's (after a reorg to a lower height) would give
/// a locktime above the chain, which nodes refuse to relay until the chain
/// catches up. If the new tip's time cannot be read then, the cache is cleared
/// rather than kept. A lower cached tip is kept: its locktime is still valid.
fn publish_tip(bit: &impl BitcoinInterface, cache: &crate::bitcoin::TipTimeCache) {
    let tip = match bit.try_chain_tip() {
        Ok(tip) => tip,
        Err(error) => {
            log::debug!("Chain tip unavailable, keeping the cached one: {}", error);
            return;
        }
    };
    let cached = cache.load();
    if cached.is_some_and(|cached| cached.tip == tip) {
        return;
    }
    match bit.try_tip_time(&tip) {
        Ok(time) => cache.store(crate::bitcoin::TipWithTime { tip, time }),
        Err(error) if cached.is_some_and(|cached| cached.tip.height > tip.height) => {
            log::debug!(
                "Tip time unavailable after the tip went down to {}, clearing the cached tip: {}",
                tip.height,
                error
            );
            cache.clear();
        }
        Err(error) => log::debug!("Tip time unavailable, keeping the cached tip: {}", error),
    }
}

#[cfg(test)]
mod tests {
    use super::publish_tip;
    use crate::{
        bitcoin::{BlockChainTip, TipTimeCache, TipWithTime},
        testutils::DummyBitcoind,
    };
    use miniscript::bitcoin::{self, hashes::Hash};

    fn tip(height: i32) -> BlockChainTip {
        BlockChainTip {
            hash: bitcoin::BlockHash::from_byte_array([height as u8; 32]),
            height,
        }
    }

    fn cached(height: i32, time: u32) -> TipTimeCache {
        let cache = TipTimeCache::default();
        cache.store(TipWithTime {
            tip: tip(height),
            time,
        });
        cache
    }

    #[test]
    fn publishes_a_new_tip_with_its_time() {
        let mut bitcoind = DummyBitcoind::new();
        bitcoind.tip = tip(101);
        bitcoind.tip_timestamp = Some(2_000);
        let cache = cached(100, 1_000);
        publish_tip(&bitcoind, &cache);
        assert_eq!(
            cache.load(),
            Some(TipWithTime {
                tip: tip(101),
                time: 2_000
            })
        );
    }

    #[test]
    fn an_unchanged_tip_keeps_its_time_without_reading_it_again() {
        let mut bitcoind = DummyBitcoind::new();
        bitcoind.tip = tip(100);
        bitcoind.tip_timestamp = Some(5_000);
        let cache = cached(100, 1_000);
        publish_tip(&bitcoind, &cache);
        assert_eq!(cache.load().map(|latest| latest.time), Some(1_000));
    }

    /// After a reorg to a lower height, a kept tip would give a locktime above
    /// the chain: without the new tip's time, spends must not use it.
    #[test]
    fn a_lower_tip_without_a_time_clears_the_cache() {
        let mut bitcoind = DummyBitcoind::new();
        bitcoind.tip = tip(99);
        bitcoind.tip_timestamp = None;
        let cache = cached(100, 1_000);
        publish_tip(&bitcoind, &cache);
        assert_eq!(cache.load(), None);
    }

    /// A tip read that fails is an outage, not a new chain. The cached tip stays
    /// so the next spend does not drop its anti-fee-sniping locktime.
    #[test]
    fn an_unavailable_tip_keeps_the_cached_one() {
        let mut bitcoind = DummyBitcoind::new();
        bitcoind.chain_tip_error = Some("backend down");
        let cache = cached(100, 1_000);
        publish_tip(&bitcoind, &cache);
        assert_eq!(
            cache.load(),
            Some(TipWithTime {
                tip: tip(100),
                time: 1_000
            })
        );
    }

    /// A higher tip without a time keeps the older one: its locktime is lower
    /// than the chain's, which is still valid.
    #[test]
    fn a_higher_tip_without_a_time_keeps_the_cached_one() {
        let mut bitcoind = DummyBitcoind::new();
        bitcoind.tip = tip(101);
        bitcoind.tip_timestamp = None;
        let cache = cached(100, 1_000);
        publish_tip(&bitcoind, &cache);
        assert_eq!(
            cache.load(),
            Some(TipWithTime {
                tip: tip(100),
                time: 1_000
            })
        );
    }
}
