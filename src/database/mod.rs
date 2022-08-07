///! Database interface for Minisafe.
///!
///! Record wallet metadata, spent and unspent coins, ongoing transactions.
pub mod sqlite;

use crate::{
    bitcoin::BlockChainTip,
    database::sqlite::{schema::DbTip, SqliteConn, SqliteDb},
};

use std::sync;

use miniscript::bitcoin::util::bip32;

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

    /// Update our best chain seen.
    fn update_tip(&mut self, tip: &BlockChainTip);

    fn derivation_index(&mut self) -> bip32::ChildNumber;

    fn update_derivation_index(&mut self, index: bip32::ChildNumber);
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

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.update_tip(&tip)
    }

    fn derivation_index(&mut self) -> bip32::ChildNumber {
        self.db_wallet().deposit_derivation_index
    }

    fn update_derivation_index(&mut self, index: bip32::ChildNumber) {
        self.update_derivation_index(index)
    }
}
