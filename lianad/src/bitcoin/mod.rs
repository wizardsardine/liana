//! Interface to the Bitcoin network.
//!
//! Broadcast transactions, poll for new unspent coins, gather fee estimates.

pub mod d;
pub mod electrum;
pub mod poller;

use crate::bitcoin::d::{BitcoindError, CachedTxGetter, LSBlockEntry};
pub use d::{MempoolEntry, MempoolEntryFees, SyncProgress};
use liana::descriptors;

use std::{fmt, sync};

use miniscript::bitcoin::{self, address, bip32::ChildNumber, sighash};
use miniscript::Interpreter;

// A spent coin's outpoint together with its spend transaction's txid, height and time.
type SpentCoin = (bitcoin::OutPoint, bitcoin::Txid, i32, u32);

const COINBASE_MATURITY: i32 = 100;

/// Information about a block
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct Block {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
    pub time: u32,
}

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

/// An error met while syncing the wallet with the Bitcoin backend.
#[derive(Debug)]
pub enum SyncError {
    /// The backend is unrecoverably out of sync, for instance an Electrum server which lied
    /// to us about the state of our coins. The poller must stop polling it.
    Fatal(String),
    /// Any other error, polling the backend may be retried.
    Transient(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SyncError::Fatal(e) | SyncError::Transient(e) => write!(f, "{e}"),
        }
    }
}

/// The outcome of checking the inputs of a transaction which may spend our coins.
#[derive(Debug, PartialEq, Eq)]
pub enum InputsCheck {
    /// None of the resolved prevouts is one of our coins.
    NotOurs,
    /// Some prevout could not be resolved, so we cannot tell whether the inputs spending our
    /// coins are honestly signed.
    Unknown,
    /// Every input spending our coins carries a valid satisfaction.
    Signed,
    /// At least one input spending one of our coins carries an invalid satisfaction.
    Forged,
}

/// Check the inputs of `tx` spending prevouts for which `is_ours` returns true. The spent
/// outputs are resolved with `resolve_prevout`; one we cannot resolve makes the check
/// inconclusive, as we cannot tell whether the input is honest.
pub fn check_our_inputs(
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
    tx: &bitcoin::Transaction,
    resolve_prevout: &mut dyn FnMut(&bitcoin::OutPoint) -> Option<bitcoin::TxOut>,
    is_ours: &dyn Fn(&bitcoin::OutPoint, &bitcoin::Script) -> bool,
) -> InputsCheck {
    let mut prev_txouts: Vec<Option<bitcoin::TxOut>> = Vec::with_capacity(tx.input.len());
    let mut our_inputs = Vec::new();
    for (index, txin) in tx.input.iter().enumerate() {
        let prev_txout = resolve_prevout(&txin.previous_output);
        if prev_txout
            .as_ref()
            .is_some_and(|txout| is_ours(&txin.previous_output, &txout.script_pubkey))
        {
            our_inputs.push(index);
        }
        prev_txouts.push(prev_txout);
    }
    if our_inputs.is_empty() {
        return if prev_txouts.iter().all(Option::is_some) {
            InputsCheck::NotOurs
        } else {
            InputsCheck::Unknown
        };
    }
    let Some(prev_txouts) = prev_txouts.into_iter().collect::<Option<Vec<_>>>() else {
        return InputsCheck::Unknown;
    };
    let prevouts = sighash::Prevouts::All(&prev_txouts);
    for &index in &our_inputs {
        let txin = &tx.input[index];
        let prev_txout = &prev_txouts[index];
        let interpreter = match Interpreter::from_txdata(
            &prev_txout.script_pubkey,
            &txin.script_sig,
            &txin.witness,
            txin.sequence,
            tx.lock_time,
        ) {
            Ok(interpreter) => interpreter,
            Err(_) => return InputsCheck::Forged,
        };
        if !interpreter
            .iter(secp, tx, index, &prevouts)
            .all(|res| res.is_ok())
        {
            return InputsCheck::Forged;
        }
    }
    InputsCheck::Signed
}

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    fn genesis_block_timestamp(&self) -> Result<u32, String>;

    fn genesis_block(&self) -> BlockChainTip;

    /// Get the progress of the block chain synchronization.
    /// Returns a rounded up percentage between 0 and 1. Use the `is_synced` method to be sure the
    /// backend is completely synced to the best known tip.
    fn sync_progress(&self) -> SyncProgress;

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Get the timestamp set in the best block's header.
    fn tip_time(&self) -> Option<u32>;

    /// Check whether this former tip is part of the current best chain.
    fn is_in_chain(&self, tip: &BlockChainTip) -> bool;

    /// Sync the wallet with the current best chain.
    /// `receive_index` and `change_index` are the last derivation indices
    /// that are expected to have been used by the wallet.
    /// In case there has been a reorg, returns the common ancestor between
    /// the wallet and the reorged chain.
    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, SyncError>;

    /// Get coins received since the specified tip.
    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO>;

    /// Get all coins that were confirmed, and at what height and time. Along with "expired"
    /// unconfirmed coins (for instance whose creating transaction may have been replaced).
    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>);

    /// Get all coins that are being spent, and the spending txid.
    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)>;

    /// Get all coins that are spent with the final spend tx txid and blocktime. Along with the
    /// coins for which the spending transaction "expired" (a conflicting transaction was mined and
    /// it wasn't spending this coin).
    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>);

    /// Get the common ancestor between the Bitcoin backend's tip and the given tip.
    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip>;

    /// Broadcast this transaction to the Bitcoin P2P network
    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String>;

    /// Trigger a rescan of the block chain for transactions related to this descriptor since
    /// the given date.
    fn start_rescan(
        &mut self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String>;

    /// Rescan progress percentage. Between 0 and 1.
    fn rescan_progress(&self) -> Option<f64>;

    /// Get the last block chain tip with a timestamp below this. Timestamp must be a valid block
    /// timestamp.
    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip>;

    /// Get a transaction related to the wallet along with potential confirmation info.
    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)>;

    /// Get the details of unconfirmed transactions spending these outpoints, if any.
    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry>;

    /// Get mempool data for the given transaction.
    ///
    /// Returns `None` if the transaction is not in the mempool.
    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry>;

    /// Whether the backend served data it can no longer be trusted for, for instance a spend
    /// of our coins with invalid signatures. The poller must stop polling it. The string is
    /// the reason, for display.
    fn fatal_error(&self) -> Option<String> {
        None
    }
}

impl BitcoinInterface for d::BitcoinD {
    fn genesis_block_timestamp(&self) -> Result<u32, String> {
        Ok(self
            .get_block_stats(
                self.get_block_hash(0)
                    .expect("Genesis block hash must always be there"),
            )
            .expect("Genesis block must always be there")
            .time)
    }

    fn genesis_block(&self) -> BlockChainTip {
        let height = 0;
        let hash = self
            .get_block_hash(height)
            .expect("Genesis block hash must always be there");
        BlockChainTip { hash, height }
    }

    fn sync_progress(&self) -> SyncProgress {
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

    // The watchonly wallet handles this for us.
    fn sync_wallet(
        &mut self,
        _receive_index: ChildNumber,
        _change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, SyncError> {
        if let Some(error) = self.fatal_error() {
            return Err(SyncError::Fatal(error));
        }
        Ok(None)
    }

    fn fatal_error(&self) -> Option<String> {
        self.forged_spend().map(|txid| {
            format!("bitcoind served a spend of our coins with invalid signatures: '{txid}'.")
        })
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
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
                    is_immature,
                } = entry;
                if parent_descs
                    .iter()
                    .any(|parent_desc| descs.iter().any(|desc| desc == parent_desc))
                {
                    Some(UTxO {
                        outpoint,
                        amount,
                        block_height,
                        address: UTxOAddress::Address(address),
                        is_immature,
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
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        // The confirmed and expired coins to be returned.
        let mut confirmed = Vec::with_capacity(outpoints.len());
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);

        for op in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(&op.txid) {
                res
            } else {
                log::error!("Transaction not in wallet for coin '{}'.", op);
                continue;
            };

            // If the transaction was confirmed, mark the coin as such.
            if let Some(block) = res.block {
                // Do not mark immature coinbase deposits as confirmed until they become mature.
                if res.is_coinbase && res.confirmations < COINBASE_MATURITY {
                    log::debug!("Coin at '{}' comes from an immature coinbase transaction with {} confirmations. Not marking it as confirmed for now.", op, res.confirmations);
                    continue;
                }
                confirmed.push((*op, block.height, block.time));
                continue;
            }

            // If the transaction was dropped from the mempool, discard the coin.
            if !self.is_in_mempool(&op.txid) {
                expired.push(*op);
            }
        }

        (confirmed, expired)
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

        // bitcoind must only ever relay consensus-valid transactions. Make sure the spends of
        // our coins it reports are honestly signed, and report none of them if one isn't or if
        // we could not check it: the endpoint is not to be trusted.
        let mut verified = Vec::with_capacity(spent.len());
        for (op, txid) in spent {
            let res = match self.get_transaction(&txid) {
                Some(res) => res,
                None => {
                    log::error!(
                        "Could not get spending transaction '{}' from bitcoind.",
                        txid
                    );
                    self.record_forged_spend(&txid);
                    continue;
                }
            };
            if matches!(
                self.check_spent_tx(&txid, &res.tx, &op, outpoints),
                d::SpendCheck::Signed
            ) {
                verified.push((op, txid));
            }
        }

        verified
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        // Spend coins to be returned.
        let mut spent = Vec::with_capacity(outpoints.len());
        // Coins whose spending transaction isn't in our local mempool anymore.
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);
        // The coins whose spend we are asked about. Their spent outputs are the ones we know
        // to be ours for the sanity check below.
        let our_outpoints: Vec<bitcoin::OutPoint> = outpoints.iter().map(|(op, _)| *op).collect();

        for (op, txid) in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(txid) {
                res
            } else {
                log::error!("Could not get tx {} spending coin {}.", txid, op);
                continue;
            };

            // bitcoind must only ever relay consensus-valid transactions: make sure this spend
            // of our coin is honestly signed, and report none of them if it isn't or if we
            // could not check it.
            if !matches!(
                self.check_spent_tx(txid, &res.tx, op, &our_outpoints),
                d::SpendCheck::Signed
            ) {
                continue;
            }

            // If the transaction was confirmed, mark it as such.
            if let Some(block) = res.block {
                spent.push((*op, *txid, block.height, block.time));
                continue;
            }

            // If a conflicting transaction was confirmed instead, replace the txid of the
            // spender for this coin with it and mark it as confirmed.
            let conflict = res.conflicting_txs.iter().find_map(|txid| {
                tx_getter.get_transaction(txid).and_then(|tx| {
                    tx.block.and_then(|block| {
                        // Being part of our watchonly wallet isn't enough, as it could be a
                        // conflicting transaction which spends a different set of coins. Make sure
                        // it does actually spend this coin.
                        tx.tx.input.iter().find_map(|txin| {
                            if &txin.previous_output == op {
                                Some((*txid, block))
                            } else {
                                None
                            }
                        })
                    })
                })
            });
            if let Some((txid, block)) = conflict {
                // The conflicting transaction spends our coin too: sanity check it like the one
                // it conflicts with before reporting it as the confirmed spender.
                if tx_getter.get_transaction(&txid).map(|res| {
                    matches!(
                        self.check_spent_tx(&txid, &res.tx, op, &our_outpoints),
                        d::SpendCheck::Signed
                    )
                }) != Some(true)
                {
                    continue;
                }
                spent.push((*op, txid, block.height, block.time));
                continue;
            }

            // If the transaction was not confirmed, a conflicting transaction spending this coin
            // too wasn't mined, but still isn't in our mempool anymore, mark the spend as expired.
            if !self.is_in_mempool(txid) {
                expired.push(*op);
            }
        }

        (spent, expired)
    }

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        let mut stats = self.get_block_stats(tip.hash)?;
        let mut ancestor = *tip;

        while stats.confirmations == -1 {
            stats = self.get_block_stats(stats.previous_blockhash?)?;
            ancestor = BlockChainTip {
                hash: stats.blockhash,
                height: stats.height,
            };
        }

        Some(ancestor)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        match self.broadcast_tx(tx) {
            Ok(()) => Ok(()),
            Err(BitcoindError::Server(e)) => Err(e.to_string()),
            // We assume the Bitcoin backend doesn't fail, so it must be a JSONRPC error.
            Err(e) => panic!(
                "Unexpected Bitcoin error when broadcast transaction: '{}'.",
                e
            ),
        }
    }

    fn start_rescan(
        &mut self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        // FIXME: in theory i think this could potentially fail to actually start the rescan.
        self.start_rescan(desc, timestamp)
            .map_err(|e| e.to_string())
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.tip_before_timestamp(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        let tip = self.chain_tip();
        Some(self.get_block_stats(tip.hash)?.time)
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.get_transaction(txid).map(|res| (res.tx, res.block))
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.mempool_txs_spending_prevouts(outpoints)
            .into_iter()
            .filter_map(|txid| self.mempool_entry(&txid))
            .collect()
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.mempool_entry(txid)
    }
}

impl BitcoinInterface for electrum::Electrum {
    fn genesis_block_timestamp(&self) -> Result<u32, String> {
        self.client()
            .genesis_block_timestamp()
            .map_err(|e| e.to_string())
    }

    fn fatal_error(&self) -> Option<String> {
        self.fatal_error()
    }

    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, SyncError> {
        self.sync_wallet(receive_index, change_index).map_err(|e| {
            let error = e.to_string();
            match e {
                electrum::ElectrumError::MaliciousServer(_)
                | electrum::ElectrumError::Client(electrum::client::Error::Disconnected) => {
                    SyncError::Fatal(error)
                }
                electrum::ElectrumError::Client(_)
                | electrum::ElectrumError::GenesisHashMismatch(..) => SyncError::Transient(error),
            }
        })
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        _descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
        // Get those wallet coins that are either unconfirmed or have a confirmation height
        // after tip. The poller will then discard any that had already been received.
        self.wallet_coins(None)
            .values()
            .filter_map(|c| {
                let height = c.block_info.map(|info| info.height);
                if height.filter(|h| *h <= tip.height).is_some() {
                    None
                } else {
                    Some(UTxO {
                        outpoint: c.outpoint,
                        block_height: height,
                        amount: c.amount,
                        address: UTxOAddress::DerivIndex(c.derivation_index, c.is_change),
                        is_immature: c.is_immature,
                    })
                }
            })
            .collect()
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        let wallet_coins = &self.wallet_coins(Some(outpoints));
        let mut confirmed = Vec::new();
        let mut expired = Vec::new();
        for op in outpoints {
            if let Some(w_c) = wallet_coins.get(op) {
                if let Some(block) = w_c.block_info {
                    if w_c.is_immature {
                        log::debug!(
                            "Coin at '{}' comes from an immature coinbase transaction at \
                            block height {}. Not marking it as confirmed for now.",
                            op,
                            block.height
                        );
                        continue;
                    }
                    confirmed.push((w_c.outpoint, block.height, block.time));
                }
            } else {
                expired.push(*op);
            }
        }
        (confirmed, expired)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        let wallet_coins = &self.wallet_coins(Some(outpoints));
        outpoints
            .iter()
            .filter_map(|op| {
                if let Some(w_c) = wallet_coins.get(op) {
                    w_c.spend_txid.map(|txid| (w_c.outpoint, txid))
                } else {
                    None
                }
            })
            .collect()
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        let ops: Vec<_> = outpoints.iter().map(|(op, _)| op).copied().collect();
        let wallet_coins = &self.wallet_coins(Some(&ops));
        let mut spent = Vec::new();
        let mut expired_spending = Vec::new();

        for (op, spend_txid) in outpoints {
            if let Some(w_c) = wallet_coins.get(op) {
                if w_c.spend_txid != Some(*spend_txid) {
                    expired_spending.push(*op);
                }
                if let Some(block) = w_c.spend_block {
                    spent.push((*op, *spend_txid, block.height, block.time));
                }
            }
        }
        (spent, expired_spending)
    }

    fn genesis_block(&self) -> BlockChainTip {
        self.client()
            .genesis_block()
            .expect("Genesis block must always be there")
    }

    fn chain_tip(&self) -> BlockChainTip {
        // We want the wallet's local chain tip after syncing.
        self.wallet_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        // Return `false` if no block at same height as `tip`
        // is in wallet's local chain.
        self.is_in_wallet_chain(*tip).unwrap_or_default()
    }

    /// FIXME: make the Bitcoin backend interface higher level. See the comment in the poller next
    /// to the `sync_wallet()` call.
    fn common_ancestor(&self, _tip: &BlockChainTip) -> Option<BlockChainTip> {
        unreachable!("The common ancestor is returned in `sync_wallet()`. If no reorg was detected then, this method will never be called on an Electrum backend.")
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        match self.client().broadcast_tx(tx) {
            Ok(_txid) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.wallet_transaction(txid)
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.client().mempool_entry(txid).ok()?
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.client()
            .mempool_spenders(outpoints)
            .unwrap_or_default()
    }

    fn sync_progress(&self) -> SyncProgress {
        // Always return 100% for now since the API is bitcoind-specific to mean "blocks/headers".
        // But in the future it would be nice to inform the user about the progress of the sync
        // if it takes a few dozen seconds.
        let blocks = self.chain_tip().height as u64;
        SyncProgress::new(1.0, blocks, blocks)
    }

    fn start_rescan(
        &mut self,
        _desc: &descriptors::LianaDescriptor,
        _timestamp: u32,
    ) -> Result<(), String> {
        self.trigger_rescan();
        Ok(())
    }

    fn rescan_progress(&self) -> Option<f64> {
        // Until we sync we're at 0%. After the sync, we're at 100%.
        self.is_rescanning().then_some(0.0)
    }

    fn block_before_date(&self, _timestamp: u32) -> Option<BlockChainTip> {
        Some(self.genesis_block())
    }

    fn tip_time(&self) -> Option<u32> {
        self.client().tip_time().ok()
    }
}

// FIXME: do we need to repeat the entire trait implementation? Isn't there a nicer way?
impl BitcoinInterface for sync::Arc<sync::Mutex<dyn BitcoinInterface + 'static>> {
    fn genesis_block_timestamp(&self) -> Result<u32, String> {
        self.lock().unwrap().genesis_block_timestamp()
    }

    fn genesis_block(&self) -> BlockChainTip {
        self.lock().unwrap().genesis_block()
    }

    fn sync_progress(&self) -> SyncProgress {
        self.lock().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.lock().unwrap().chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.lock().unwrap().is_in_chain(tip)
    }

    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, SyncError> {
        self.lock()
            .unwrap()
            .sync_wallet(receive_index, change_index)
    }

    fn fatal_error(&self) -> Option<String> {
        self.lock().unwrap().fatal_error()
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
        self.lock().unwrap().received_coins(tip, descs)
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
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
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        self.lock().unwrap().spent_coins(outpoints)
    }

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        self.lock().unwrap().common_ancestor(tip)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        self.lock().unwrap().broadcast_tx(tx)
    }

    fn start_rescan(
        &mut self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        self.lock().unwrap().start_rescan(desc, timestamp)
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.lock().unwrap().rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.lock().unwrap().block_before_date(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        self.lock().unwrap().tip_time()
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.lock().unwrap().wallet_transaction(txid)
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.lock().unwrap().mempool_spenders(outpoints)
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.lock().unwrap().mempool_entry(txid)
    }
}

// FIXME: We could avoid this type (and all the conversions entailing allocations) if bitcoind
// exposed the derivation index from the parent descriptor in the LSB result.
#[derive(Debug, Clone)]
pub struct UTxO {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub block_height: Option<i32>,
    pub address: UTxOAddress,
    pub is_immature: bool,
}

/// Details about the UTXO address.
#[derive(Debug, Clone)]
pub enum UTxOAddress {
    Address(bitcoin::Address<address::NetworkUnchecked>),
    /// Derivation index and whether it is from the change descriptor.
    DerivIndex(ChildNumber, bool),
}

#[derive(Debug, Clone, Copy)]
pub struct BlockInfo {
    pub height: i32,
    pub time: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Coin {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub derivation_index: ChildNumber,
    pub is_change: bool,
    pub is_immature: bool,
    pub block_info: Option<BlockInfo>,
    pub spend_txid: Option<bitcoin::Txid>,
    pub spend_block: Option<BlockInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::HashMap, str::FromStr};

    use miniscript::bitcoin::{
        absolute,
        hashes::Hash,
        key::PrivateKey,
        secp256k1::{self, SecretKey},
        transaction::Version,
        Amount, Network, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Txid, Witness,
    };

    fn secret_key(byte: u8) -> SecretKey {
        SecretKey::from_str(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    /// A transaction spending `input_ops`, with one dummy output.
    fn spending_tx(input_ops: &[OutPoint]) -> bitcoin::Transaction {
        bitcoin::Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: input_ops
                .iter()
                .map(|op| TxIn {
                    previous_output: *op,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    /// The p2wpkh output owned by `sk`, along with the outpoint it is spent at.
    fn our_coin(sk: &SecretKey, op: OutPoint) -> (OutPoint, TxOut) {
        let secp = secp256k1::Secp256k1::signing_only();
        let pk =
            bitcoin::PublicKey::from_private_key(&secp, &PrivateKey::new(*sk, Network::Regtest));
        (
            op,
            TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::new_p2wpkh(&pk.wpubkey_hash().unwrap()),
            },
        )
    }

    fn foreign_coin(op: OutPoint) -> (OutPoint, TxOut) {
        (
            op,
            TxOut {
                value: Amount::from_sat(200_000),
                script_pubkey: ScriptBuf::from_hex(
                    "512053a1f6e444c9bd1e2b5c2b2c6b8c9bd1e2b5c2b2c6b8c9bd1e2b5c2b2c6b8ca1",
                )
                .unwrap(),
            },
        )
    }

    /// Sign input `index` of `tx` with `sk`, assuming it spends the p2wpkh output of `sk`.
    fn sign_p2wpkh(
        tx: &mut bitcoin::Transaction,
        index: usize,
        prevout: &TxOut,
        sk: &SecretKey,
        sighash_type: sighash::EcdsaSighashType,
    ) {
        let sighash = sighash::SighashCache::new(&*tx)
            .p2wpkh_signature_hash(index, &prevout.script_pubkey, prevout.value, sighash_type)
            .unwrap();
        let secp = secp256k1::Secp256k1::signing_only();
        let mut sig = secp
            .sign_ecdsa(
                &secp256k1::Message::from_digest(sighash.to_byte_array()),
                sk,
            )
            .serialize_der()
            .to_vec();
        sig.push(sighash_type.to_u32() as u8);
        let pk_bytes = sk.public_key(&secp).serialize().to_vec();
        tx.input[index].witness = Witness::from_slice(&[sig, pk_bytes]);
    }

    fn outpoint(txid_hex: &str, vout: u32) -> OutPoint {
        OutPoint::new(Txid::from_str(txid_hex).unwrap(), vout)
    }

    #[test]
    fn valid_spend_of_our_coin() {
        let sk = secret_key(0x01);
        let (our_op, our_prevout) = our_coin(
            &sk,
            outpoint(
                "1111111111111111111111111111111111111111111111111111111111111111",
                0,
            ),
        );
        let (foreign_op, foreign_prevout) = foreign_coin(outpoint(
            "2222222222222222222222222222222222222222222222222222222222222222",
            1,
        ));
        let mut tx = spending_tx(&[foreign_op, our_op]);
        sign_p2wpkh(
            &mut tx,
            1,
            &our_prevout,
            &sk,
            sighash::EcdsaSighashType::All,
        );
        let mut prevouts = HashMap::from([(foreign_op, foreign_prevout), (our_op, our_prevout)]);
        let secp = secp256k1::Secp256k1::verification_only();
        let check = check_our_inputs(
            &secp,
            &tx,
            &mut |op: &OutPoint| prevouts.remove(op),
            &|op: &OutPoint, _: &bitcoin::Script| *op == our_op,
        );
        assert_eq!(check, InputsCheck::Signed);
    }

    #[test]
    fn unknown_prevout_is_not_a_forgery() {
        let sk = secret_key(0x01);
        let (our_op, our_prevout) = our_coin(
            &sk,
            outpoint(
                "1111111111111111111111111111111111111111111111111111111111111111",
                0,
            ),
        );
        let foreign_op = outpoint(
            "2222222222222222222222222222222222222222222222222222222222222222",
            1,
        );
        let mut tx = spending_tx(&[our_op, foreign_op]);
        sign_p2wpkh(
            &mut tx,
            0,
            &our_prevout,
            &sk,
            sighash::EcdsaSighashType::All,
        );
        // The server did not provide the prevout of the foreign input.
        let mut prevouts = HashMap::from([(our_op, our_prevout)]);
        let secp = secp256k1::Secp256k1::verification_only();
        let check = check_our_inputs(
            &secp,
            &tx,
            &mut |op: &OutPoint| prevouts.remove(op),
            &|op: &OutPoint, _: &bitcoin::Script| *op == our_op,
        );
        assert_eq!(check, InputsCheck::Unknown);
    }

    #[test]
    fn invalid_satisfaction_is_a_forgery() {
        let sk = secret_key(0x01);
        let attacker_sk = secret_key(0x02);
        let (our_op, our_prevout) = our_coin(
            &sk,
            outpoint(
                "1111111111111111111111111111111111111111111111111111111111111111",
                0,
            ),
        );
        let mut tx = spending_tx(&[our_op]);
        sign_p2wpkh(
            &mut tx,
            0,
            &our_prevout,
            &attacker_sk,
            sighash::EcdsaSighashType::All,
        );
        let mut prevouts = HashMap::from([(our_op, our_prevout)]);
        let secp = secp256k1::Secp256k1::verification_only();
        let check = check_our_inputs(
            &secp,
            &tx,
            &mut |op: &OutPoint| prevouts.remove(op),
            &|op: &OutPoint, _: &bitcoin::Script| *op == our_op,
        );
        assert_eq!(check, InputsCheck::Forged);
    }

    #[test]
    fn spend_of_foreign_coins_only() {
        let (first_op, first_prevout) = foreign_coin(outpoint(
            "1111111111111111111111111111111111111111111111111111111111111111",
            0,
        ));
        let (second_op, second_prevout) = foreign_coin(outpoint(
            "2222222222222222222222222222222222222222222222222222222222222222",
            1,
        ));
        let tx = spending_tx(&[first_op, second_op]);
        let mut prevouts = HashMap::from([(first_op, first_prevout), (second_op, second_prevout)]);
        let secp = secp256k1::Secp256k1::verification_only();
        let check = check_our_inputs(
            &secp,
            &tx,
            &mut |op: &OutPoint| prevouts.remove(op),
            &|_: &OutPoint, _: &bitcoin::Script| false,
        );
        assert_eq!(check, InputsCheck::NotOurs);
    }
}
