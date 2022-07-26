///! Interface to the Bitcoin network.
///!
///! Broadcast transactions, poll for new unspent coins, gather fee estimates.
pub mod d;
pub mod poller;

use std::sync;

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    /// Get the progress of the block chain synchronization.
    /// Returns a percentage between 0 and 1.
    fn sync_progress(&self) -> f64;
}

impl BitcoinInterface for sync::Arc<sync::RwLock<d::BitcoinD>> {
    fn sync_progress(&self) -> f64 {
        self.read().unwrap().sync_progress()
    }
}
