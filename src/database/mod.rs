///! Database interface for Minisafe.
///!
///! Record wallet metadata, spent and unspent coins, ongoing transactions.
pub mod sqlite;

use crate::{
    bitcoin::BlockChainTip,
    database::sqlite::{
        schema::{DbCoin, DbTip},
        SqliteConn, SqliteDb,
    },
};

use std::{collections::HashMap, sync};

use miniscript::bitcoin::{
    self, secp256k1,
    util::{bip32, psbt::PartiallySignedTransaction as Psbt},
};

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

    fn derivation_index(&mut self) -> bip32::ChildNumber;

    fn increment_derivation_index(&mut self, secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>);

    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<bip32::ChildNumber>;

    /// Get all UTxOs.
    fn unspent_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin>;

    /// Store new UTxOs. Coins must not already be in database.
    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]);

    /// Mark a set of coins as being confirmed at a specified height.
    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32)]);

    /// Mark a set of coins as being spent by a specified txid.
    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]);

    /// Get specific coins from the database.
    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin>;

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt>;

    /// Insert a new Spend transaction or replace an existing one.
    fn store_spend(&mut self, psbt: &Psbt);

    /// List all existing Spend transactions.
    fn list_spend(&mut self) -> Vec<Psbt>;

    /// Delete a Spend transaction from database.
    fn delete_spend(&mut self, txid: &bitcoin::Txid);
}

// FIXME: if possible, avoid reallocating.
fn db_coins_into_coins(db_coins: Vec<DbCoin>) -> HashMap<bitcoin::OutPoint, Coin> {
    db_coins
        .into_iter()
        .map(|db_coin| {
            let DbCoin {
                outpoint,
                block_height,
                amount,
                derivation_index,
                spend_txid,
                ..
            } = db_coin;
            (
                outpoint,
                Coin {
                    outpoint,
                    block_height,
                    amount,
                    derivation_index,
                    spend_txid,
                },
            )
        })
        .collect()
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
        self.update_tip(&tip)
    }

    fn derivation_index(&mut self) -> bip32::ChildNumber {
        self.db_wallet().deposit_derivation_index
    }

    fn increment_derivation_index(&mut self, secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>) {
        self.increment_derivation_index(secp)
    }

    fn unspent_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin> {
        db_coins_into_coins(self.unspent_coins())
    }

    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]) {
        self.new_unspent_coins(coins)
    }

    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32)]) {
        self.confirm_coins(outpoints)
    }

    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]) {
        self.spend_coins(outpoints)
    }

    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<bip32::ChildNumber> {
        self.db_address(address)
            .map(|db_addr| db_addr.derivation_index)
    }

    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        db_coins_into_coins(self.db_coins(outpoints))
    }

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt> {
        self.db_spend(txid).map(|db_spend| db_spend.psbt)
    }

    fn store_spend(&mut self, psbt: &Psbt) {
        self.store_spend(psbt)
    }

    fn list_spend(&mut self) -> Vec<Psbt> {
        self.list_spend()
            .into_iter()
            .map(|db_spend| db_spend.psbt)
            .collect()
    }

    fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        self.delete_spend(txid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coin {
    pub outpoint: bitcoin::OutPoint,
    pub block_height: Option<i32>,
    pub amount: bitcoin::Amount,
    pub derivation_index: bip32::ChildNumber,
    pub spend_txid: Option<bitcoin::Txid>,
}

impl std::hash::Hash for Coin {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.outpoint.hash(h)
    }
}

impl Coin {
    pub fn is_confirmed(&self) -> bool {
        self.block_height.is_some()
    }

    pub fn is_spent(&self) -> bool {
        self.spend_txid.is_some()
    }
}
