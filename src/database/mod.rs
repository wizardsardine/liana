//! Database interface for Liana.
//!
//! Record wallet metadata, spent and unspent coins, ongoing transactions.

pub mod sqlite;

use crate::{
    bitcoin::BlockChainTip,
    database::sqlite::{
        schema::{DbBlockInfo, DbCoin, DbTip},
        SqliteConn, SqliteDb,
    },
};

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    iter::FromIterator,
    str::FromStr,
    sync,
};

use miniscript::bitcoin::{self, bip32, psbt::PartiallySignedTransaction as Psbt, secp256k1};

pub trait DatabaseInterface: Send {
    fn connection(&self) -> Box<dyn DatabaseConnection>;
}

impl DatabaseInterface for SqliteDb {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        Box::new(self.connection().expect("Database must be available"))
    }
}

// FIXME: do we need to repeat the entire trait implemenation? Isn't there a nicer way?
impl DatabaseInterface for sync::Arc<sync::Mutex<dyn DatabaseInterface>> {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        self.lock().unwrap().connection()
    }
}

pub trait DatabaseConnection {
    /// Get the tip of the best chain we've seen.
    fn chain_tip(&mut self) -> Option<BlockChainTip>;

    /// The network we are operating on.
    fn network(&mut self) -> bitcoin::Network;

    /// Update our best chain seen.
    fn update_tip(&mut self, tip: &BlockChainTip);

    /// Get the derivation index for the next receiving address
    fn receive_index(&mut self) -> bip32::ChildNumber;

    /// Set the derivation index for the next receiving address
    fn set_receive_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    );

    /// Get the derivation index for the next change address
    fn change_index(&mut self) -> bip32::ChildNumber;

    /// Set the derivation index for the next change address
    fn set_change_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    );

    /// Get the timestamp at which to start rescaning from, if any.
    fn rescan_timestamp(&mut self) -> Option<u32>;

    /// Set a timestamp at which to start rescaning the block chain from.
    fn set_rescan(&mut self, timestamp: u32);

    /// Mark the rescan as complete.
    fn complete_rescan(&mut self);

    /// Get the derivation index for this address, as well as whether this address is change.
    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<(bip32::ChildNumber, bool)>;

    /// Get all our coins, past or present, spent or not.
    fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin>;

    /// List coins that are being spent and whose spending transaction is still unconfirmed.
    fn list_spending_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin>;

    /// Store new UTxOs. Coins must not already be in database.
    fn new_unspent_coins(&mut self, coins: &[Coin]);

    /// Remove some UTxOs from the database.
    fn remove_coins(&mut self, coins: &[bitcoin::OutPoint]);

    /// Mark a set of coins as being confirmed at a specified height and block time.
    /// NOTE: if the coin comes from an immature coinbase transaction, this will mark it as mature.
    /// Immature coinbase deposits must not be confirmed before they are 100 blocks deep in the
    /// chain.
    fn confirm_coins(&mut self, outpoints: &[(bitcoin::OutPoint, i32, u32)]);

    /// Mark a set of coins as being spent by a specified txid of a pending transaction.
    fn spend_coins(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]);

    /// Mark a set of coins as spent by a specified txid at a specified block time.
    fn confirm_spend(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid, i32, u32)]);

    /// Get specific coins from the database.
    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin>;

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt>;

    /// Insert a new Spend transaction or replace an existing one.
    fn store_spend(&mut self, psbt: &Psbt);

    /// List all existing Spend transactions, along with an optional last update timestamp.
    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>)>;

    /// Delete a Spend transaction from database.
    fn delete_spend(&mut self, txid: &bitcoin::Txid);

    /// Update, for a set of items (as key), their label (as value). A `None` value deletes the
    /// label.
    fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>);

    fn labels(&mut self, labels: &HashSet<LabelItem>) -> HashMap<String, String>;

    /// Mark the given tip as the new best seen block. Update stored data accordingly.
    fn rollback_tip(&mut self, new_tip: &BlockChainTip);

    /// Retrieve a limited list of txids that where deposited or spent between the start and end timestamps (inclusive bounds)
    fn list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid>;
}

impl DatabaseConnection for SqliteConn {
    fn chain_tip(&mut self) -> Option<BlockChainTip> {
        match self.db_tip() {
            DbTip {
                block_height: Some(height),
                block_hash: Some(hash),
                ..
            } => Some(BlockChainTip { height, hash }),
            _ => None,
        }
    }

    fn network(&mut self) -> bitcoin::Network {
        self.db_tip().network
    }

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.update_tip(tip)
    }

    fn receive_index(&mut self) -> bip32::ChildNumber {
        self.db_wallet().deposit_derivation_index
    }

    fn set_receive_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.set_derivation_index(index, false, secp)
    }

    fn change_index(&mut self) -> bip32::ChildNumber {
        self.db_wallet().change_derivation_index
    }

    fn set_change_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.set_derivation_index(index, true, secp)
    }

    fn rescan_timestamp(&mut self) -> Option<u32> {
        self.db_wallet().rescan_timestamp
    }

    fn set_rescan(&mut self, timestamp: u32) {
        self.set_wallet_rescan_timestamp(timestamp)
    }

    fn complete_rescan(&mut self) {
        self.complete_wallet_rescan()
    }

    fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        self.coins(statuses, outpoints)
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn list_spending_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin> {
        self.list_spending_coins()
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]) {
        self.new_unspent_coins(coins)
    }

    fn remove_coins(&mut self, outpoints: &[bitcoin::OutPoint]) {
        self.remove_coins(outpoints)
    }

    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32, u32)]) {
        self.confirm_coins(outpoints)
    }

    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]) {
        self.spend_coins(outpoints)
    }

    fn confirm_spend<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid, i32, u32)]) {
        self.confirm_spend(outpoints)
    }

    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<(bip32::ChildNumber, bool)> {
        self.db_address(address).map(|db_addr| {
            (
                db_addr.derivation_index,
                // We only compare address strings in case `assume_checked()` uses a different network.
                // E.g. An unchecked signet address would have its network set to testnet and so comparing
                // to a signet `Address` would never match.
                address.to_string() == db_addr.change_address.assume_checked().to_string(),
            )
        })
    }

    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        self.db_coins(outpoints)
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt> {
        self.db_spend(txid).map(|db_spend| db_spend.psbt)
    }

    fn store_spend(&mut self, psbt: &Psbt) {
        self.store_spend(psbt)
    }

    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>)> {
        self.list_spend()
            .into_iter()
            .map(|db_spend| (db_spend.psbt, db_spend.updated_at))
            .collect()
    }

    fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        self.delete_spend(txid)
    }

    fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>) {
        self.update_labels(items)
    }

    fn labels(&mut self, items: &HashSet<LabelItem>) -> HashMap<String, String> {
        let labels = self.db_labels(items);
        HashMap::from_iter(labels.into_iter().map(|label| (label.item, label.value)))
    }

    fn rollback_tip(&mut self, new_tip: &BlockChainTip) {
        self.rollback_tip(new_tip)
    }

    fn list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid> {
        self.db_list_txids(start, end, limit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockInfo {
    pub height: i32,
    pub time: u32,
}

impl From<DbBlockInfo> for BlockInfo {
    fn from(b: DbBlockInfo) -> BlockInfo {
        BlockInfo {
            height: b.height,
            time: b.time,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Coin {
    pub outpoint: bitcoin::OutPoint,
    pub is_immature: bool,
    pub block_info: Option<BlockInfo>,
    pub amount: bitcoin::Amount,
    pub derivation_index: bip32::ChildNumber,
    pub is_change: bool,
    pub spend_txid: Option<bitcoin::Txid>,
    pub spend_block: Option<BlockInfo>,
}

impl std::convert::From<DbCoin> for Coin {
    fn from(db_coin: DbCoin) -> Coin {
        let DbCoin {
            outpoint,
            is_immature,
            block_info,
            amount,
            derivation_index,
            is_change,
            spend_txid,
            spend_block,
            ..
        } = db_coin;
        Coin {
            outpoint,
            is_immature,
            block_info: block_info.map(BlockInfo::from),
            amount,
            derivation_index,
            is_change,
            spend_txid,
            spend_block: spend_block.map(BlockInfo::from),
        }
    }
}

impl Coin {
    pub fn is_confirmed(&self) -> bool {
        self.block_info.is_some()
    }

    pub fn is_spent(&self) -> bool {
        self.spend_txid.is_some()
    }
}

/// Possible (mutually exclusive) status of a coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoinStatus {
    /// Has not yet been included in a block and has no spend transaction.
    Unconfirmed,
    /// Has been included in a block and has no spend transaction.
    Confirmed,
    /// Has an unconfirmed spend transaction, but coin itself may not yet have been included in a block.
    Spending,
    /// Has a confirmed spend transaction.
    Spent,
}

impl CoinStatus {
    pub fn from_arg(s: &str) -> Option<CoinStatus> {
        match s {
            "unconfirmed" => Some(CoinStatus::Unconfirmed),
            "confirmed" => Some(CoinStatus::Confirmed),
            "spending" => Some(CoinStatus::Spending),
            "spent" => Some(CoinStatus::Spent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LabelItem {
    Address(bitcoin::Address),
    Txid(bitcoin::Txid),
    OutPoint(bitcoin::OutPoint),
}

impl From<bitcoin::Address> for LabelItem {
    fn from(value: bitcoin::Address) -> Self {
        Self::Address(value)
    }
}

impl From<bitcoin::Txid> for LabelItem {
    fn from(value: bitcoin::Txid) -> Self {
        Self::Txid(value)
    }
}

impl From<bitcoin::OutPoint> for LabelItem {
    fn from(value: bitcoin::OutPoint) -> Self {
        Self::OutPoint(value)
    }
}

impl Display for LabelItem {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LabelItem::Address(a) => write!(f, "{}", a),
            LabelItem::Txid(a) => write!(f, "{}", a),
            LabelItem::OutPoint(a) => write!(f, "{}", a),
        }
    }
}

impl LabelItem {
    pub fn from_str(s: &str, network: bitcoin::Network) -> Option<LabelItem> {
        if let Ok(addr) = bitcoin::Address::from_str(s) {
            if !addr.is_valid_for_network(network) {
                None
            } else {
                Some(LabelItem::Address(addr.assume_checked()))
            }
        } else if let Ok(txid) = bitcoin::Txid::from_str(s) {
            Some(LabelItem::Txid(txid))
        } else if let Ok(outpoint) = bitcoin::OutPoint::from_str(s) {
            Some(LabelItem::OutPoint(outpoint))
        } else {
            None
        }
    }
}
