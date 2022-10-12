///! Interface to the Bitcoin network.
///!
///! Broadcast transactions, poll for new unspent coins, gather fee estimates.
pub mod d;
pub mod poller;

use d::LSBlockEntry;

use std::collections::HashMap;
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

    /// Get all coins that were confirmed, and at what height and time.
    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, i32, u32)>;

    /// Get all coins that are being spent, and the spending txid.
    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)>;

    /// Get all coins that are spent with the final spend tx txid and blocktime.
    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, u32)>;
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

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, i32, u32)> {
        let mut confirmed = Vec::with_capacity(outpoints.len());

        for op in outpoints {
            // TODO: batch those calls to gettransaction
            if let Some(res) = self.get_transaction(&op.txid) {
                if let Some(h) = res.block_height {
                    if let Some(t) = res.block_time {
                        confirmed.push((*op, h, t));
                    }
                }
            } else {
                log::error!("Transaction not in wallet for coin '{}'.", op);
            }
        }

        confirmed
    }

    fn spending_coins(
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

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, u32)> {
        let mut spent = Vec::with_capacity(outpoints.len());

        let mut cache: HashMap<bitcoin::Txid, Option<d::GetTxRes>> = HashMap::new();
        for (op, txid) in outpoints {
            let tx: Option<&d::GetTxRes> = match cache.get(txid) {
                Some(tx) => tx.as_ref(),
                None => {
                    let tx = self.get_transaction(txid);
                    cache.insert(*txid, tx);
                    cache.get(txid).unwrap().as_ref()
                }
            };

            // There is an immutable borrow on the cache, these txs will be added once it is
            // dropped.
            let mut txs_to_cache: Vec<(bitcoin::Txid, Option<d::GetTxRes>)> = Vec::new();

            if let Some(tx) = tx {
                if let Some(block_time) = tx.block_time {
                    // TODO: make both block time and height under the same Option.
                    assert!(tx.block_height.is_some());
                    spent.push((*op, *txid, block_time))
                } else if !tx.conflicting_txs.is_empty() {
                    for txid in &tx.conflicting_txs {
                        let tx: Option<&d::GetTxRes> = match cache.get(txid) {
                            Some(tx) => tx.as_ref(),
                            None => {
                                let tx = self.get_transaction(&txid);
                                txs_to_cache.push((*txid, tx));
                                txs_to_cache.last().unwrap().1.as_ref()
                            }
                        };
                        if let Some(tx) = tx {
                            if let Some(block_height) = tx.block_height {
                                if block_height > 1 {
                                    spent.push((
                                        *op,
                                        *txid,
                                        tx.block_time.expect("Spend is confirmed"),
                                    ))
                                }
                            }
                        }
                    }
                }
            }

            for (txid, res) in txs_to_cache {
                cache.insert(txid, res);
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

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, i32, u32)> {
        self.lock().unwrap().confirmed_coins(outpoints)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        self.lock().unwrap().spending_coins(outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, u32)> {
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
