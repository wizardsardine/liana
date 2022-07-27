///! Interface to the Bitcoin network.
///!
///! Broadcast transactions, poll for new unspent coins, gather fee estimates.
pub mod d;
pub mod poller;

use std::sync;

use miniscript::bitcoin;

/// Information about the best block in the chain
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BlockChainTip {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
}

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    /// Get the progress of the block chain synchronization.
    /// Returns a percentage between 0 and 1.
    fn sync_progress(&self) -> f64;

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Check whether this former tip is part of the current best chain.
    fn is_in_chain(&self, tip: &BlockChainTip) -> bool;
}

impl BitcoinInterface for sync::Arc<sync::RwLock<d::BitcoinD>> {
    fn sync_progress(&self) -> f64 {
        self.read().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.read().unwrap().chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.read()
            .unwrap()
            .get_block_hash(tip.height)
            .map(|bh| bh == tip.hash)
            .unwrap_or(false)
    }
}
