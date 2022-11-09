///! Interface to the Bitcoin network.
///!
///! Broadcast transactions, poll for new unspent coins, gather fee estimates.
pub mod d;
pub mod poller;

use crate::{
    bitcoin::d::{BitcoindError, LSBlockEntry},
    descriptors,
};

use std::{collections::HashMap, error, fmt, sync};

use miniscript::bitcoin;

/// Error occuring when querying our Bitcoin backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitcoinError {
    Broadcast(String),
}

impl fmt::Display for BitcoinError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BitcoinError::Broadcast(reason) => {
                write!(f, "Failed to broadcast transaction: '{}'", reason)
            }
        }
    }
}

impl error::Error for BitcoinError {}

/// Information about the best block in the chain
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BlockChainTip {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
}

impl fmt::Display for BlockChainTip {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({},{})", self.height, self.hash)
    }
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
    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::InheritanceDescriptor],
    ) -> Vec<UTxO>;

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
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)>;

    /// Get the common ancestor between the Bitcoin backend's tip and the given tip.
    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip>;

    /// Broadcast this transaction to the Bitcoin P2P network
    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), BitcoinError>;
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

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::InheritanceDescriptor],
    ) -> Vec<UTxO> {
        // TODO: don't assume only a single descriptor is loaded on the wo wallet
        let lsb_res = self.list_since_block(&tip.hash);

        lsb_res
            .received_coins
            .into_iter()
            .filter_map(|entry| {
                let LSBlockEntry {
                    outpoint,
                    amount,
                    block_height,
                    address,
                    parent_descs,
                } = entry;
                if parent_descs
                    .iter()
                    .any(|parent_desc| descs.iter().any(|desc| desc == parent_desc))
                {
                    Some(UTxO {
                        outpoint,
                        amount,
                        block_height,
                        address,
                    })
                } else {
                    None
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
            if self.is_spent(op) {
                let spending_txid = if let Some(txid) = self.get_spender_txid(op) {
                    txid
                } else {
                    // TODO: better handling of this edge case.
                    log::error!(
                        "Could not get spender of '{}'. Not reporting it as spending.",
                        op
                    );
                    continue;
                };

                spent.push((*op, spending_txid));
            }
        }

        spent
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)> {
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
                if let Some(block_height) = tx.block_height {
                    // TODO: make both block time and height under the same Option.
                    assert!(tx.block_height.is_some());
                    spent.push((
                        *op,
                        *txid,
                        block_height,
                        tx.block_time.expect("Confirmed tx."),
                    ));
                } else if !tx.conflicting_txs.is_empty() {
                    for txid in &tx.conflicting_txs {
                        let tx: Option<&d::GetTxRes> = match cache.get(txid) {
                            Some(tx) => tx.as_ref(),
                            None => {
                                let tx = self.get_transaction(txid);
                                txs_to_cache.push((*txid, tx));
                                txs_to_cache.last().unwrap().1.as_ref()
                            }
                        };
                        if let Some(tx) = tx {
                            if let Some(block_height) = tx.block_height {
                                spent.push((
                                    *op,
                                    *txid,
                                    block_height,
                                    tx.block_time.expect("Spend is confirmed"),
                                ))
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

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        let mut stats = self.get_block_stats(tip.hash);
        let mut ancestor = *tip;

        while stats.confirmations == -1 {
            stats = self.get_block_stats(stats.previous_blockhash?);
            ancestor = BlockChainTip {
                hash: stats.blockhash,
                height: stats.height,
            };
        }

        Some(ancestor)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), BitcoinError> {
        match self.broadcast_tx(tx) {
            Ok(()) => Ok(()),
            Err(BitcoindError::Server(e)) => Err(BitcoinError::Broadcast(e.to_string())),
            // We assume the Bitcoin backend doesn't fail, so it must be a JSONRPC error.
            Err(e) => panic!(
                "Unexpected Bitcoin error when broadcast transaction: '{}'.",
                e
            ),
        }
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

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::InheritanceDescriptor],
    ) -> Vec<UTxO> {
        self.lock().unwrap().received_coins(tip, descs)
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
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)> {
        self.lock().unwrap().spent_coins(outpoints)
    }

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        self.lock().unwrap().common_ancestor(tip)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), BitcoinError> {
        self.lock().unwrap().broadcast_tx(tx)
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
