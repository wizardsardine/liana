///! Database interface for Minisafe.
///!
///! Record wallet metadata, spent and unspent coins, ongoing transactions.
pub mod sqlite;

use crate::{
    bitcoin::BlockChainTip,
    database::sqlite::{schema::DbTip, SqliteConn, SqliteDb},
};

pub trait DatabaseInterface: Send {
    fn connection(&self) -> Box<dyn DatabaseConnection>;
}

impl DatabaseInterface for SqliteDb {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        Box::new(self.connection().expect("Database must be available"))
    }
}

pub trait DatabaseConnection {
    /// Get the tip of the best chain we've seen.
    fn chain_tip(&mut self) -> Option<BlockChainTip>;

    /// Update our best chain seen.
    fn update_tip(&mut self, tip: &BlockChainTip);
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
}
