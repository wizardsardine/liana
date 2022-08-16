///! Interface to the Bitcoin network.
///!
///! Broadcast transactions, poll for new unspent coins, gather fee estimates.
pub mod d;
pub mod poller;

use d::LSBlockEntry;

use std::sync;

use miniscript::bitcoin::{self, hashes::Hash};

/// Information about the best block in the chain
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BlockChainTip {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
}

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    fn genesis_block(&self) -> BlockChainTip;

    /// Get the progress of the block chain synchronization.
    /// Returns a percentage between 0 and 1.
    fn sync_progress(&self) -> f64;

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Check whether this former tip is part of the current best chain.
    fn is_in_chain(&self, tip: &BlockChainTip) -> bool;

    /// Get coins received since the specified tip.
    fn received_coins(&self, tip: &BlockChainTip) -> Vec<UTxO>;

    /// Get all coins that were confirmed, and at what height.
    fn confirmed_coins(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, i32)>;

    /// Get all coins that were spent, and the spending txid.
    fn spent_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)>;
}

impl BitcoinInterface for d::BitcoinD {
    fn genesis_block(&self) -> BlockChainTip {
        let height = 0;
        let hash = self
            .get_block_hash(height)
            .expect("Genesis block hash must always be there");
        BlockChainTip { hash, height }
    }

    fn sync_progress(&self) -> f64 {
        self.sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.get_block_hash(tip.height)
            .map(|bh| bh == tip.hash)
            .unwrap_or(false)
    }

    fn received_coins(&self, tip: &BlockChainTip) -> Vec<UTxO> {
        // TODO: don't assume only a single descriptor is loaded on the wo wallet
        let lsb_res = self.list_since_block(&tip.hash);

        lsb_res
            .received_coins
            .into_iter()
            .map(|entry| {
                let LSBlockEntry {
                    outpoint,
                    amount,
                    block_height,
                    address,
                } = entry;
                UTxO {
                    outpoint,
                    amount,
                    block_height,
                    address,
                }
            })
            .collect()
    }

    fn confirmed_coins(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, i32)> {
        let mut confirmed = Vec::with_capacity(outpoints.len());

        for op in outpoints {
            // TODO: batch those calls to gettransaction
            if let Some(res) = self.get_transaction(&op.txid) {
                if let Some(h) = res.block_height {
                    confirmed.push((*op, h));
                }
            } else {
                log::error!("Transaction not in wallet for coin '{}'.", op);
            }
        }

        confirmed
    }

    fn spent_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        let mut spent = Vec::with_capacity(outpoints.len());

        for op in outpoints {
            if self.is_spent(&op) {
                let spending_txid = if let Some(txid) = self.get_spender_txid(&op) {
                    txid
                } else {
                    // TODO: better handling of this edge case.
                    log::error!(
                        "Could not get spender of '{}'. Using a dummy spending txid.",
                        op
                    );
                    bitcoin::Txid::from_slice(&[0; 32][..]).unwrap()
                };
                spent.push((*op, spending_txid));
            }
        }

        spent
    }
}

// FIXME: do we need to repeat the entire trait implemenation? Isn't there a nicer way?
impl BitcoinInterface for sync::Arc<sync::Mutex<dyn BitcoinInterface + 'static>> {
    fn genesis_block(&self) -> BlockChainTip {
        self.lock().unwrap().genesis_block()
    }

    fn sync_progress(&self) -> f64 {
        self.lock().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.lock().unwrap().chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.lock().unwrap().is_in_chain(tip)
    }

    fn received_coins(&self, tip: &BlockChainTip) -> Vec<UTxO> {
        self.lock().unwrap().received_coins(tip)
    }

    fn confirmed_coins(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, i32)> {
        self.lock().unwrap().confirmed_coins(outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        self.lock().unwrap().spent_coins(outpoints)
    }
}

// FIXME: We could avoid this type (and all the conversions entailing allocations) if bitcoind
// exposed the derivation index from the parent descriptor in the LSB result.
#[derive(Debug, Clone)]
pub struct UTxO {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub block_height: Option<i32>,
    pub address: bitcoin::Address,
}
