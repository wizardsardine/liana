//! Implementation of the database interface using SQLite.
//!
//! We use a bundled SQLite that is compiled with SQLITE_THREADSAFE. Sqlite.org states:
//! > Multi-thread. In this mode, SQLite can be safely used by multiple threads provided that
//! > no single database connection is used simultaneously in two or more threads.
//!
//! We leverage SQLite's `unlock_notify` feature to synchronize writes accross connection. More
//! about it at https://sqlite.org/unlock_notify.html.

pub mod schema;
mod utils;

use crate::{
    bitcoin::BlockChainTip,
    database::{
        sqlite::{
            schema::{
                DbAddress, DbCoin, DbLabel, DbLabelledKind, DbSpendTransaction, DbTip, DbWallet,
                SCHEMA,
            },
            utils::{
                create_fresh_db, curr_timestamp, db_exec, db_query, db_tx_query, db_version,
                maybe_apply_migration, LOOK_AHEAD_LIMIT,
            },
        },
        Coin, CoinStatus, LabelItem,
    },
    descriptors::LianaDescriptor,
};

use std::{
    cmp,
    collections::{HashMap, HashSet},
    convert::TryInto,
    fmt, io, path,
};

use miniscript::bitcoin::{
    self, bip32,
    consensus::encode,
    hashes::{sha256, Hash},
    psbt::Psbt,
    secp256k1,
};

const DB_VERSION: i64 = 3;

#[derive(Debug)]
pub enum SqliteDbError {
    FileCreation(io::Error),
    FileNotFound(path::PathBuf),
    UnsupportedVersion(i64),
    InvalidNetwork(bitcoin::Network),
    DescriptorMismatch(Box<LianaDescriptor>),
    Rusqlite(rusqlite::Error),
}

impl std::fmt::Display for SqliteDbError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            SqliteDbError::FileCreation(e) => {
                write!(f, "Error when create SQLite database file: '{}'", e)
            }
            SqliteDbError::FileNotFound(p) => {
                write!(f, "SQLite database file not found at '{}'.", p.display())
            }
            SqliteDbError::UnsupportedVersion(v) => {
                write!(f, "Unsupported database version '{}'.", v)
            }
            SqliteDbError::InvalidNetwork(net) => {
                write!(f, "Database was created for network '{}'.", net)
            }
            SqliteDbError::DescriptorMismatch(desc) => {
                write!(f, "Database descriptor mismatch: '{}'.", desc)
            }
            SqliteDbError::Rusqlite(e) => write!(f, "SQLite error: '{}'", e),
        }
    }
}

impl std::error::Error for SqliteDbError {}

impl From<io::Error> for SqliteDbError {
    fn from(e: io::Error) -> Self {
        SqliteDbError::FileCreation(e)
    }
}

impl From<rusqlite::Error> for SqliteDbError {
    fn from(e: rusqlite::Error) -> Self {
        SqliteDbError::Rusqlite(e)
    }
}

// In Bitcoin land, txids are usually displayed in reverse byte order. This is what rust-bitcoin
// implements as `fmt::Display` for `bitcoin::Txid`. However, we store them as raw bytes in the
// database and it so happens we sometimes have to look for a txid in hex, in which case we want
// the "frontward" hex serialization. This is a hack to implement it.
#[derive(Debug, Clone, Copy)]
struct FrontwardHexTxid(bitcoin::Txid);

impl fmt::Display for FrontwardHexTxid {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{:x}",
            // sha256 isn't displayed in reverse byte order (contrary to sha256d).
            sha256::Hash::from_byte_array(self.0.to_byte_array())
        )
    }
}

#[derive(Debug, Clone)]
pub struct FreshDbOptions {
    pub(self) bitcoind_network: bitcoin::Network,
    pub(self) main_descriptor: LianaDescriptor,
    pub(self) schema: &'static str,
    pub(self) version: i64,
}

impl FreshDbOptions {
    pub fn new(
        bitcoind_network: bitcoin::Network,
        main_descriptor: LianaDescriptor,
    ) -> FreshDbOptions {
        FreshDbOptions {
            bitcoind_network,
            main_descriptor,
            schema: SCHEMA,
            version: DB_VERSION,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteDb {
    db_path: path::PathBuf,
}

impl SqliteDb {
    /// Instanciate an SQLite database either from an existing database file or by creating a fresh
    /// one.
    pub fn new(
        db_path: path::PathBuf,
        fresh_options: Option<FreshDbOptions>,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) -> Result<SqliteDb, SqliteDbError> {
        // Create the database if needed, and make sure the db file exists.
        if let Some(options) = fresh_options {
            create_fresh_db(&db_path, options, secp)?;
            log::info!("Created a fresh database at {}.", db_path.display());
        }
        if !db_path.exists() {
            return Err(SqliteDbError::FileNotFound(db_path));
        }

        log::info!("Checking if the database needs upgrading.");
        maybe_apply_migration(&db_path)?;

        Ok(SqliteDb { db_path })
    }

    /// Get a new connection to the database.
    pub fn connection(&self) -> Result<SqliteConn, SqliteDbError> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(60))?;
        Ok(SqliteConn { conn })
    }

    /// Perform startup sanity checks.
    pub fn sanity_check(
        &self,
        bitcoind_network: bitcoin::Network,
        main_descriptor: &LianaDescriptor,
    ) -> Result<(), SqliteDbError> {
        let mut conn = self.connection()?;

        // At this point any migration must have been applied.
        let db_version = conn.db_version();
        if db_version != DB_VERSION {
            return Err(SqliteDbError::UnsupportedVersion(db_version));
        }

        // The config and the db should be on the same network.
        let db_tip = conn.db_tip();
        if db_tip.network != bitcoind_network {
            return Err(SqliteDbError::InvalidNetwork(db_tip.network));
        }

        // The config and db descriptors must match!
        let db_wallet = conn.db_wallet();
        if &db_wallet.main_descriptor != main_descriptor {
            return Err(SqliteDbError::DescriptorMismatch(
                db_wallet.main_descriptor.into(),
            ));
        }

        Ok(())
    }
}

// We only support single wallet. The id of the wallet row is always 1.
const WALLET_ID: i64 = 1;

pub struct SqliteConn {
    conn: rusqlite::Connection,
}

impl SqliteConn {
    pub fn db_version(&mut self) -> i64 {
        db_version(&mut self.conn).expect("db must not fail")
    }

    /// Get the network tip.
    pub fn db_tip(&mut self) -> DbTip {
        db_query(
            &mut self.conn,
            "SELECT * FROM tip",
            rusqlite::params![],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
        .pop()
        .expect("There is always a row in the tip table")
    }

    /// Get the information about the wallet.
    pub fn db_wallet(&mut self) -> DbWallet {
        db_query(
            &mut self.conn,
            "SELECT * FROM wallets",
            rusqlite::params![],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
        .pop()
        .expect("There is always a row in the wallet table")
    }

    /// Update the network tip.
    pub fn update_tip(&mut self, tip: &BlockChainTip) {
        db_exec(&mut self.conn, |db_tx| {
            db_tx
                .execute(
                    "UPDATE tip SET blockheight = (?1), blockhash = (?2)",
                    rusqlite::params![tip.height, tip.hash[..].to_vec()],
                )
                .map(|_| ())
        })
        .expect("Database must be available")
    }

    /// Set the derivation index for receiving or change addresses.
    ///
    /// This will populate the address->deriv_index mapping with all the new entries between the
    /// former and new gap limit indexes.
    pub fn set_derivation_index(
        &mut self,
        index: bip32::ChildNumber,
        change: bool,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        let network = self.db_tip().network;

        db_exec(&mut self.conn, |db_tx| {
            let db_wallet: DbWallet =
                db_tx_query(db_tx, "SELECT * FROM wallets", rusqlite::params![], |row| {
                    row.try_into()
                })?
                .pop()
                .expect("There is always a row in the wallet table");

            // Make sure we don't set a lower derivation index. This can happen since the
            // derivation is set outside the atomic transaction. So there may be a race between say
            // the Bitcoin poller thread and the JSONRPC commands thread.
            if (change && index <= db_wallet.change_derivation_index) || (!change && index <= db_wallet.deposit_derivation_index) {
                // It was already set at a higher index.
                return Ok(());
            }

            // First of all set the derivation index
            let index_u32: u32 = index.into();
            if change {
                db_tx.execute(
                    "UPDATE wallets SET change_derivation_index = (?1)",
                    rusqlite::params![index_u32],
                )?;
            } else {
                db_tx.execute(
                    "UPDATE wallets SET deposit_derivation_index = (?1)",
                    rusqlite::params![index_u32],
                )?;
            }

            // Now if this new index is higher than the highest of our current derivation indexes,
            // populate the addresses mapping for derivation indexes between our previous "gap
            // limit index" and the new one.
            let curr_highest_index = cmp::max(
                db_wallet.deposit_derivation_index,
                db_wallet.change_derivation_index,
            ).into();
            if index_u32 > curr_highest_index {
                let receive_desc = db_wallet.main_descriptor.receive_descriptor();
                let change_desc = db_wallet.main_descriptor.change_descriptor();

                for index in curr_highest_index + 1..=index_u32 {
                    let la_index = index + LOOK_AHEAD_LIMIT - 1;
                    let receive_addr = receive_desc.derive(la_index.into(), secp).address(network);
                    let change_addr = change_desc.derive(la_index.into(), secp).address(network);
                    db_tx.execute(
                        "INSERT INTO addresses (receive_address, change_address, derivation_index) VALUES (?1, ?2, ?3)",
                        rusqlite::params![receive_addr.to_string(), change_addr.to_string(), la_index],
                    )?;
                }

            }

            Ok(())
        })
        .expect("Database must be available")
    }

    pub fn set_wallet_rescan_timestamp(&mut self, timestamp: u32) {
        db_exec(&mut self.conn, |db_tx| {
            // NOTE: this will need to be updated if we ever implement multi-wallet support
            db_tx
                .execute(
                    "UPDATE wallets SET rescan_timestamp = (?1)",
                    rusqlite::params![timestamp],
                )
                .map(|_| ())
        })
        .expect("Database must be available")
    }

    /// Drop the rescan timestamp, and set it as the wallet creation timestamp if it
    /// predates it.
    ///
    /// # Panics
    /// - If called while rescan_timestamp is not set
    pub fn complete_wallet_rescan(&mut self) {
        let db_wallet = self.db_wallet();
        let new_timestamp = cmp::min(
            db_wallet.rescan_timestamp.expect("Must be set"),
            db_wallet.timestamp,
        );

        db_exec(&mut self.conn, |db_tx| {
            // NOTE: this will need to be updated if we ever implement multi-wallet support
            db_tx
                .execute(
                    "UPDATE wallets SET timestamp = (?1), rescan_timestamp = NULL",
                    rusqlite::params![new_timestamp],
                )
                .map(|_| ())
        })
        .expect("Database must be available");
    }

    /// Get all the coins from DB, optionally filtered by coin status and/or outpoint.
    pub fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<DbCoin> {
        let status_condition = statuses
            .iter()
            .map(|c| {
                format!(
                    "({})",
                    match c {
                        CoinStatus::Unconfirmed => {
                            "blocktime IS NULL AND spend_txid IS NULL"
                        }
                        CoinStatus::Confirmed => {
                            "blocktime IS NOT NULL AND spend_txid IS NULL"
                        }
                        CoinStatus::Spending => {
                            "spend_txid IS NOT NULL AND spend_block_time IS NULL"
                        }
                        CoinStatus::Spent => "spend_block_time IS NOT NULL",
                    }
                )
            })
            .collect::<Vec<String>>()
            .join(" OR ");
        // SELECT * FROM coins WHERE (txid, vout) IN ((txidA, voutA), (txidB, voutB));
        let op_condition = if !outpoints.is_empty() {
            let mut cond = "(txid, vout) IN (VALUES ".to_string();
            for (i, outpoint) in outpoints.iter().enumerate() {
                // NOTE: SQLite doesn't know Satoshi decided txids would be displayed as little-endian
                // hex.
                cond += &format!(
                    "(x'{}', {})",
                    FrontwardHexTxid(outpoint.txid),
                    outpoint.vout
                );
                if i != outpoints.len() - 1 {
                    cond += ", ";
                }
            }
            cond += ")";
            cond
        } else {
            String::new()
        };
        let where_clause = if !status_condition.is_empty() && !op_condition.is_empty() {
            format!(" WHERE ({}) AND ({})", status_condition, op_condition)
        } else if status_condition.is_empty() && !op_condition.is_empty() {
            format!(" WHERE {}", op_condition)
        } else if !status_condition.is_empty() && op_condition.is_empty() {
            format!(" WHERE {}", status_condition)
        } else {
            String::new()
        };
        let query = format!("SELECT * FROM coins{}", where_clause);
        db_query(&mut self.conn, &query, rusqlite::params![], |row| {
            row.try_into()
        })
        .expect("Db must not fail")
    }

    /// List coins that are being spent and whose spending transaction is still unconfirmed.
    pub fn list_spending_coins(&mut self) -> Vec<DbCoin> {
        self.coins(&[CoinStatus::Spending], &[])
    }

    // FIXME: don't take the whole coin, we don't need it.
    /// Store new, unconfirmed and unspent, coins.
    /// Will panic if given a coin that is already in DB.
    pub fn new_unspent_coins<'a>(&mut self, coins: impl IntoIterator<Item = &'a Coin>) {
        db_exec(&mut self.conn, |db_tx| {
            for coin in coins {
                let deriv_index: u32 = coin.derivation_index.into();
                db_tx.execute(
                    "INSERT INTO coins (wallet_id, txid, vout, amount_sat, derivation_index, is_change, is_immature) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        WALLET_ID,
                        coin.outpoint.txid[..].to_vec(),
                        coin.outpoint.vout,
                        coin.amount.to_sat(),
                        deriv_index,
                        coin.is_change,
                        coin.is_immature,
                    ],
                )?;
            }
            Ok(())
        })
        .expect("Database must be available")
    }

    /// Remove a set of coins from the database.
    pub fn remove_coins(&mut self, outpoints: &[bitcoin::OutPoint]) {
        db_exec(&mut self.conn, |db_tx| {
            for outpoint in outpoints {
                db_tx.execute(
                    "DELETE FROM coins WHERE txid = ?1 AND vout = ?2",
                    rusqlite::params![outpoint.txid[..].to_vec(), outpoint.vout,],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as confirmed.
    ///
    /// NOTE: this will also mark the coin as mature if it originates from an immature coinbase
    /// deposit.
    pub fn confirm_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, i32, u32)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, height, time) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET blockheight = ?1, blocktime = ?2, is_immature = 0 WHERE txid = ?3 AND vout = ?4",
                    rusqlite::params![height, time, outpoint.txid[..].to_vec(), outpoint.vout,],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as spending.
    pub fn spend_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, bitcoin::Txid)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, spend_txid) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET spend_txid = ?1 WHERE txid = ?2 AND vout = ?3",
                    rusqlite::params![
                        spend_txid[..].to_vec(),
                        outpoint.txid[..].to_vec(),
                        outpoint.vout,
                    ],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as not being spent.
    pub fn unspend_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a bitcoin::OutPoint>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for outpoint in outpoints {
                db_tx.execute(
                    "UPDATE coins SET spend_txid = NULL, spend_block_height = NULL, spend_block_time = NULL WHERE txid = ?1 AND vout = ?2",
                    rusqlite::params![
                        outpoint.txid[..].to_vec(),
                        outpoint.vout,
                    ],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark the Spend transaction of a given set of coins as being confirmed at a given
    /// block.
    pub fn confirm_spend<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, bitcoin::Txid, i32, u32)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, spend_txid, height, time) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET spend_txid = ?1, spend_block_height = ?2, spend_block_time = ?3 WHERE txid = ?4 AND vout = ?5",
                    rusqlite::params![
                        spend_txid[..].to_vec(),
                        height,
                        time,
                        outpoint.txid[..].to_vec(),
                        outpoint.vout,
                    ],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    pub fn db_address(&mut self, address: &bitcoin::Address) -> Option<DbAddress> {
        db_query(
            &mut self.conn,
            "SELECT * FROM addresses WHERE receive_address = ?1 OR change_address = ?1",
            rusqlite::params![address.to_string()],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
        .pop()
    }

    pub fn db_coins(&mut self, outpoints: &[bitcoin::OutPoint]) -> Vec<DbCoin> {
        self.coins(&[], outpoints)
    }

    pub fn db_spend(&mut self, txid: &bitcoin::Txid) -> Option<DbSpendTransaction> {
        db_query(
            &mut self.conn,
            "SELECT * FROM spend_transactions WHERE txid = ?1",
            rusqlite::params![txid[..].to_vec()],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
        .pop()
    }

    /// Insert a new Spend transaction or replace an existing one.
    pub fn store_spend(&mut self, psbt: &Psbt) {
        let txid = &psbt.unsigned_tx.txid()[..].to_vec();

        db_exec(&mut self.conn, |db_tx| {
            db_tx.execute(
                "INSERT into spend_transactions (psbt, txid, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT DO UPDATE SET psbt=excluded.psbt",
                rusqlite::params![psbt.serialize(), txid, curr_timestamp()],
            )?;
            Ok(())
        })
        .expect("Db must not fail");
    }

    pub fn list_spend(&mut self) -> Vec<DbSpendTransaction> {
        db_query(
            &mut self.conn,
            "SELECT * FROM spend_transactions",
            rusqlite::params![],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
    }

    pub fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>) {
        db_exec(&mut self.conn, |db_tx| {
            for (labelled, kind, value) in items
                .iter()
                .map(|(a, v)| {
                     match a {
                         LabelItem::Address(a) =>(a.to_string(), DbLabelledKind::Address, v),
                         LabelItem::Txid(a) =>(a.to_string(), DbLabelledKind::Txid, v),
                         LabelItem::OutPoint(a) =>(a.to_string(), DbLabelledKind::OutPoint, v),
                     }
                }) {
                if let Some(value) = value {
                    db_tx.execute(
                        "INSERT INTO labels (wallet_id, item, item_kind, value) VALUES (?1, ?2, ?3, ?4) \
                        ON CONFLICT DO UPDATE SET value=excluded.value",
                        rusqlite::params![WALLET_ID, labelled, kind as i64, value],
                    )?;
                } else {
                    db_tx.execute(
                        "DELETE FROM labels WHERE wallet_id = ?1 AND item = ?2",
                        rusqlite::params![WALLET_ID, labelled],
                    )?;
                }
            }
            Ok(())
        })
        .expect("Db must not fail")
    }

    pub fn db_labels(&mut self, items: &HashSet<LabelItem>) -> Vec<DbLabel> {
        let query = format!(
            "SELECT * FROM labels where item in ({})",
            items
                .iter()
                .map(|a| format!("'{}'", a))
                .collect::<Vec<String>>()
                .join(",")
        );
        db_query(&mut self.conn, &query, rusqlite::params![], |row| {
            row.try_into()
        })
        .expect("Db must not fail")
    }

    /// Retrieves a limited and ordered list of transactions ids that happened during the given
    /// range.
    pub fn db_list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid> {
        db_query(
            &mut self.conn,
            "SELECT DISTINCT(txid) FROM ( \
                SELECT * from ( \
                    SELECT txid, blocktime AS date FROM coins \
                    WHERE blocktime >= (?1) \
                    AND blocktime <= (?2) \
                    ORDER BY blocktime \
                ) \
                UNION \
                SELECT * FROM (
                    SELECT spend_txid AS txid, spend_block_time AS date FROM coins \
                    WHERE spend_block_time >= (?1) \
                    AND spend_block_time <= (?2) \
                    ORDER BY spend_block_time \
                ) \
                ORDER BY date DESC LIMIT (?3) \
            )",
            rusqlite::params![start, end, limit],
            |row| {
                let txid: Vec<u8> = row.get(0)?;
                let txid: bitcoin::Txid =
                    encode::deserialize(&txid).expect("We only store valid txids");
                Ok(txid)
            },
        )
        .expect("Db must not fail")
    }

    pub fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        db_exec(&mut self.conn, |db_tx| {
            db_tx.execute(
                "DELETE FROM spend_transactions WHERE txid = ?1",
                rusqlite::params![txid[..].to_vec()],
            )?;
            Ok(())
        })
        .expect("Db must not fail");
    }

    // TODO: mark coinbase deposits that were mature and became immature as such.
    /// Unconfirm all data that was marked as being confirmed *after* the given chain
    /// tip, and set it as our new best block seen.
    ///
    /// This includes:
    /// - Coins (coinbase deposits that became immature isn't currently implemented)
    /// - Spending transactions confirmation
    /// - Tip
    ///
    /// This will have to be updated if we are to add new fields based on block data
    /// in the database eventually.
    pub fn rollback_tip(&mut self, new_tip: &BlockChainTip) {
        db_exec(&mut self.conn, |db_tx| {
            db_tx.execute(
                "UPDATE coins SET blockheight = NULL, blocktime = NULL, spend_block_height = NULL, spend_block_time = NULL WHERE blockheight > ?1",
                rusqlite::params![new_tip.height],
            )?;
            db_tx.execute(
                "UPDATE coins SET spend_block_height = NULL, spend_block_time = NULL WHERE spend_block_height > ?1",
                rusqlite::params![new_tip.height],
            )?;
            db_tx.execute(
                "UPDATE tip SET blockheight = (?1), blockhash = (?2)",
                rusqlite::params![new_tip.height, new_tip.hash[..].to_vec()],
            )?;
            Ok(())
        })
        .expect("Db must not fail");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{BlockInfo, DbBlockInfo};
    use crate::testutils::*;
    use std::{
        collections::{HashMap, HashSet},
        fs, path,
        str::FromStr,
    };

    use bitcoin::{bip32, hashes::Hash};

    // The database schema used by the first versions of Liana (database version 0). Used to test
    // migrations starting from the first version.
    const V0_SCHEMA: &str = "\
CREATE TABLE version (
    version INTEGER NOT NULL
);

/* About the Bitcoin network. */
CREATE TABLE tip (
    network TEXT NOT NULL,
    blockheight INTEGER,
    blockhash BLOB
);

/* This stores metadata about our wallet. We only support single wallet for
 * now (and the foreseeable future).
 *
 * The 'timestamp' field is the creation date of the wallet. We guarantee to have seen all
 * information related to our descriptor(s) that occured after this date.
 * The optional 'rescan_timestamp' field is a the timestamp we need to rescan the chain
 * for events related to our descriptor(s) from.
 */
CREATE TABLE wallets (
    id INTEGER PRIMARY KEY NOT NULL,
    timestamp INTEGER NOT NULL,
    main_descriptor TEXT NOT NULL,
    deposit_derivation_index INTEGER NOT NULL,
    change_derivation_index INTEGER NOT NULL,
    rescan_timestamp INTEGER
);

/* Our (U)TxOs.
 *
 * The 'spend_block_height' and 'spend_block.time' are only present if the spending
 * transaction for this coin exists and was confirmed.
 */
CREATE TABLE coins (
    id INTEGER PRIMARY KEY NOT NULL,
    wallet_id INTEGER NOT NULL,
    blockheight INTEGER,
    blocktime INTEGER,
    txid BLOB NOT NULL,
    vout INTEGER NOT NULL,
    amount_sat INTEGER NOT NULL,
    derivation_index INTEGER NOT NULL,
    is_change BOOLEAN NOT NULL CHECK (is_change IN (0,1)),
    spend_txid BLOB,
    spend_block_height INTEGER,
    spend_block_time INTEGER,
    UNIQUE (txid, vout),
    FOREIGN KEY (wallet_id) REFERENCES wallets (id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

/* A mapping from descriptor address to derivation index. Necessary until
 * we can get the derivation index from the parent descriptor from bitcoind.
 */
CREATE TABLE addresses (
    receive_address TEXT NOT NULL UNIQUE,
    change_address TEXT NOT NULL UNIQUE,
    derivation_index INTEGER NOT NULL UNIQUE
);

/* Transactions we created that spend some of our coins. */
CREATE TABLE spend_transactions (
    id INTEGER PRIMARY KEY NOT NULL,
    psbt BLOB UNIQUE NOT NULL,
    txid BLOB UNIQUE NOT NULL
);
";

    fn psbt_from_str(psbt_str: &str) -> Psbt {
        Psbt::from_str(psbt_str).unwrap()
    }

    fn dummy_options() -> FreshDbOptions {
        let desc_str = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs";
        let main_descriptor = LianaDescriptor::from_str(desc_str).unwrap();
        FreshDbOptions::new(bitcoin::Network::Bitcoin, main_descriptor)
    }

    fn dummy_db() -> (
        path::PathBuf,
        FreshDbOptions,
        secp256k1::Secp256k1<secp256k1::VerifyOnly>,
        SqliteDb,
    ) {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let secp = secp256k1::Secp256k1::verification_only();

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("lianad.sqlite3")]
            .iter()
            .collect();
        let options = dummy_options();
        let db = SqliteDb::new(db_path, Some(options.clone()), &secp).unwrap();

        (tmp_dir, options, secp, db)
    }

    #[test]
    fn db_startup_sanity_checks() {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let secp = secp256k1::Secp256k1::verification_only();

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("lianad.sqlite3")]
            .iter()
            .collect();
        assert!(SqliteDb::new(db_path.clone(), None, &secp)
            .unwrap_err()
            .to_string()
            .contains("database file not found"));

        let options = dummy_options();

        let db = SqliteDb::new(db_path.clone(), Some(options.clone()), &secp).unwrap();
        db.sanity_check(bitcoin::Network::Testnet, &options.main_descriptor)
            .unwrap_err()
            .to_string()
            .contains("Database was created for network");
        fs::remove_file(&db_path).unwrap();
        let other_desc_str = "wsh(andor(pk([aabbccdd]tpubDExU4YLJkyQ9RRbVScQq2brFxWWha7WmAUByPWyaWYwmcTv3Shx8aHp6mVwuE5n4TeM4z5DTWGf2YhNPmXtfvyr8cUDVvA3txdrFnFgNdF7/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))";
        let other_desc = LianaDescriptor::from_str(other_desc_str).unwrap();
        let db = SqliteDb::new(db_path.clone(), Some(options.clone()), &secp).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &other_desc)
            .unwrap_err()
            .to_string()
            .contains("Database descriptor mismatch");
        fs::remove_file(&db_path).unwrap();
        // TODO: version check

        let db = SqliteDb::new(db_path.clone(), Some(options.clone()), &secp).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &options.main_descriptor)
            .unwrap();
        let db = SqliteDb::new(db_path, None, &secp).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &options.main_descriptor)
            .unwrap();

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn db_tip_update() {
        let (tmp_dir, options, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();
            let db_tip = conn.db_tip();
            assert!(
                db_tip.block_hash.is_none()
                    && db_tip.block_height.is_none()
                    && db_tip.network == options.bitcoind_network
            );
            let new_tip = BlockChainTip {
                height: 746756,
                hash: bitcoin::BlockHash::from_str(
                    "00000000000000000006d50e4c9fd269ddf690c94f422dff85e96f1a84b3a615",
                )
                .unwrap(),
            };
            conn.update_tip(&new_tip);
            let db_tip = conn.db_tip();
            assert_eq!(db_tip.block_height.unwrap(), new_tip.height);
            assert_eq!(db_tip.block_hash.unwrap(), new_tip.hash);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn db_labels_update() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let txid_str = "0c62a990d20d54429e70859292e82374ba6b1b951a3ab60f26bb65fee5724ff7";
            let txid = LabelItem::from_str(txid_str, bitcoin::Network::Bitcoin).unwrap();
            let mut items = HashSet::new();
            items.insert(txid.clone());

            let mut conn = db.connection().unwrap();
            let db_labels = conn.db_labels(&items);
            assert!(db_labels.is_empty());

            let mut txids_labels = HashMap::new();
            txids_labels.insert(txid.clone(), Some("hello".to_string()));

            conn.update_labels(&txids_labels);

            let db_labels = conn.db_labels(&items);
            assert_eq!(db_labels[0].value, "hello");

            txids_labels.insert(txid.clone(), Some("hello again".to_string()));
            conn.update_labels(&txids_labels);

            let db_labels = conn.db_labels(&items);
            assert_eq!(db_labels[0].value, "hello again");

            // Now delete the label by passing a None value.
            *txids_labels.get_mut(&txid).unwrap() = None;
            conn.update_labels(&txids_labels);
            let db_labels = conn.db_labels(&items);
            assert!(db_labels.is_empty());
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn db_coins() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // Necessarily empty at first.
            assert!(conn.coins(&[], &[]).is_empty());

            // Add one unconfirmed coin.
            let outpoint_a = bitcoin::OutPoint::from_str(
                "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
            )
            .unwrap();
            let coin_a = Coin {
                outpoint: outpoint_a,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(10000),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_a]);
            // We can query by status and/or outpoint.
            assert!([
                conn.coins(&[], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_a]),
                conn.coins(&[], &[outpoint_a]),
                conn.db_coins(&[outpoint_a]),
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_a.outpoint));
            // It will not be returned if we filter for other statuses.
            assert!(conn
                .coins(
                    &[
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent
                    ],
                    &[]
                )
                .is_empty());
            // Filtering also for its outpoint will still not return it if status does not match.
            assert!(conn
                .coins(
                    &[
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent
                    ],
                    &[outpoint_a]
                )
                .is_empty());

            // Add a second coin.
            let outpoint_b = bitcoin::OutPoint::from_str(
                "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571936f:12",
            )
            .unwrap();
            let coin_b = Coin {
                outpoint: outpoint_b,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(1111),
                derivation_index: bip32::ChildNumber::from_normal_idx(103).unwrap(),
                is_change: true,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_b]);
            // Both coins are unconfirmed.
            assert!([
                conn.coins(&[], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_a, outpoint_b]),
                conn.coins(&[], &[outpoint_a, outpoint_b]),
                conn.db_coins(&[outpoint_a, outpoint_b]),
            ]
            .iter()
            .all(|c| c.len() == 2
                && c[0].outpoint == coin_a.outpoint
                && c[1].outpoint == coin_b.outpoint));
            // We can filter for just the first coin.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_a]),
                conn.coins(&[], &[outpoint_a]),
                conn.db_coins(&[outpoint_a])
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_a.outpoint));
            // Or we can filter for just the second coin.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_b]),
                conn.coins(&[], &[outpoint_b]),
                conn.db_coins(&[outpoint_b])
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_b.outpoint));
            // There are no coins with other statuses.
            assert!(conn
                .coins(
                    &[
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent
                    ],
                    &[]
                )
                .is_empty());
            // Now if we confirm one, it'll be marked as such.
            conn.confirm_coins(&[(coin_a.outpoint, 174500, 174500)]);
            assert!([
                conn.coins(&[CoinStatus::Confirmed], &[]),
                conn.coins(&[CoinStatus::Confirmed], &[outpoint_a]),
                conn.coins(&[], &[outpoint_a]),
                conn.db_coins(&[outpoint_a]),
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_a.outpoint));
            // We can get both confirmed and unconfirmed.
            assert!([
                conn.coins(&[], &[]),
                conn.coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[]),
                conn.coins(
                    &[CoinStatus::Unconfirmed, CoinStatus::Confirmed],
                    &[outpoint_a, outpoint_b]
                ),
                conn.coins(&[], &[outpoint_a, outpoint_b]),
                conn.db_coins(&[outpoint_a, outpoint_b]),
            ]
            .iter()
            .all(|c| c.len() == 2
                && c[0].outpoint == coin_a.outpoint
                && c[1].outpoint == coin_b.outpoint));

            // Now if we spend one, it'll be marked as such.
            conn.spend_coins(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
            )]);
            assert!([
                conn.coins(&[CoinStatus::Spending], &[]),
                conn.coins(&[CoinStatus::Spending], &[outpoint_a]),
                conn.coins(&[], &[outpoint_a]),
                conn.list_spending_coins(),
                conn.db_coins(&[outpoint_a])
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_a.outpoint));
            // The second coin is still unconfirmed.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_b]),
                conn.coins(&[], &[outpoint_b]),
                conn.db_coins(&[outpoint_b])
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_b.outpoint));

            // Now we confirm the spend.
            conn.confirm_spend(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
                128_097,
                3_000_000,
            )]);
            // The coin no longer has spending status.
            assert!([
                conn.coins(&[CoinStatus::Spending], &[]),
                conn.coins(&[CoinStatus::Spending], &[outpoint_a]),
                conn.list_spending_coins(),
            ]
            .iter()
            .all(|res| res.is_empty()));

            // Both coins are still in DB.
            assert!([
                conn.coins(&[], &[]),
                conn.coins(&[CoinStatus::Unconfirmed, CoinStatus::Spent], &[]),
                conn.coins(
                    &[CoinStatus::Unconfirmed, CoinStatus::Spent],
                    &[outpoint_a, outpoint_b]
                ),
                conn.coins(&[], &[outpoint_a, outpoint_b]),
                conn.db_coins(&[outpoint_a, outpoint_b]),
            ]
            .iter()
            .all(|c| c.len() == 2
                && c[0].outpoint == coin_a.outpoint
                && c[1].outpoint == coin_b.outpoint));

            // Add a third and fourth coin.
            let outpoint_c = bitcoin::OutPoint::from_str(
                "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571937a:42",
            )
            .unwrap();
            let coin_c = Coin {
                outpoint: outpoint_c,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(30000),
                derivation_index: bip32::ChildNumber::from_normal_idx(4103).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            let outpoint_d = bitcoin::OutPoint::from_str(
                "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571937a:43",
            )
            .unwrap();
            let coin_d = Coin {
                outpoint: outpoint_d,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(40000),
                derivation_index: bip32::ChildNumber::from_normal_idx(4104).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_c, coin_d]);

            // We can get all three unconfirmed coins with different status/outpoint filters.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(
                    &[CoinStatus::Unconfirmed],
                    &[outpoint_b, outpoint_c, outpoint_d]
                ),
                conn.coins(&[], &[outpoint_b, outpoint_c, outpoint_d]),
                conn.db_coins(&[outpoint_b, outpoint_c, outpoint_d]),
            ]
            .iter()
            .all(|coin| coin.len() == 3
                && coin[0].outpoint == coin_b.outpoint
                && coin[1].outpoint == coin_c.outpoint
                && coin[2].outpoint == coin_d.outpoint));

            // We can also get two of the three unconfirmed coins by filtering for their outpoints.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_b, outpoint_c]),
                conn.coins(&[], &[outpoint_b, outpoint_c]),
                conn.db_coins(&[outpoint_b, outpoint_c]),
            ]
            .iter()
            .all(|coin| coin.len() == 2
                && coin[0].outpoint == coin_b.outpoint
                && coin[1].outpoint == coin_c.outpoint));

            // Now spend second coin, even though it is still unconfirmed.
            conn.spend_coins(&[(
                coin_b.outpoint,
                bitcoin::Txid::from_slice(&[1; 32][..]).unwrap(),
            )]);
            // The coin shows as spending.
            assert!([
                conn.coins(&[CoinStatus::Spending], &[]),
                conn.coins(&[CoinStatus::Spending], &[outpoint_b]),
                conn.coins(&[], &[outpoint_b]),
                conn.list_spending_coins(),
                conn.db_coins(&[outpoint_b])
            ]
            .iter()
            .all(|res| res.len() == 1 && res[0].outpoint == coin_b.outpoint));

            // Now confirm the third coin.
            conn.confirm_coins(&[(coin_c.outpoint, 175500, 175500)]);

            // We now only have one unconfirmed coin.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(
                    &[CoinStatus::Unconfirmed],
                    &[outpoint_a, outpoint_b, outpoint_c, outpoint_d]
                ),
                conn.coins(&[], &[outpoint_d]),
                conn.db_coins(&[outpoint_d]),
            ]
            .iter()
            .all(|c| c.len() == 1 && c[0].outpoint == coin_d.outpoint));

            // There is now one coin for each status.
            assert!([
                conn.coins(&[CoinStatus::Unconfirmed], &[]),
                conn.coins(&[CoinStatus::Unconfirmed], &[outpoint_d]),
                conn.coins(&[CoinStatus::Confirmed], &[]),
                conn.coins(&[CoinStatus::Confirmed], &[outpoint_c]),
                conn.coins(&[CoinStatus::Spending], &[]),
                conn.coins(&[CoinStatus::Spending], &[outpoint_b]),
                conn.coins(&[CoinStatus::Spent], &[]),
                conn.coins(&[CoinStatus::Spent], &[outpoint_a]),
                conn.coins(&[], &[outpoint_a]),
                conn.coins(&[], &[outpoint_b]),
                conn.coins(&[], &[outpoint_c]),
                conn.coins(&[], &[outpoint_d]),
            ]
            .iter()
            .map(|c| c.len())
            .all(|length| length == 1));
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn db_coins_update() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // Necessarily empty at first.
            assert!(conn.coins(&[], &[]).is_empty());

            // Add one, we'll get it.
            let coin_a = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                )
                .unwrap(),
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(98765),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_a]);
            assert_eq!(conn.coins(&[], &[])[0].outpoint, coin_a.outpoint);

            // We can also remove it. Say the unconfirmed tx that created it got replaced.
            conn.remove_coins(&[coin_a.outpoint]);
            assert!(conn.coins(&[], &[]).is_empty());

            // Add it back for the rest of the test.
            conn.new_unspent_coins(&[coin_a]);

            // We can query it by its outpoint
            let coins = conn.db_coins(&[coin_a.outpoint]);
            assert_eq!(coins.len(), 1);
            assert_eq!(coins[0].outpoint, coin_a.outpoint);

            // It is unconfirmed.
            assert_eq!(
                conn.coins(&[CoinStatus::Unconfirmed], &[])[0].outpoint,
                coin_a.outpoint
            );
            assert!(conn
                .coins(
                    &[
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent
                    ],
                    &[]
                )
                .is_empty());

            // Add a second one (this one is change), we'll get both.
            let coin_b = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571936f:12",
                )
                .unwrap(),
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(1111),
                derivation_index: bip32::ChildNumber::from_normal_idx(103).unwrap(),
                is_change: true,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_b]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .coins(&[], &[])
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_a.outpoint));
            assert!(outpoints.contains(&coin_b.outpoint));

            // We can query both by their outpoints
            let coins = conn.db_coins(&[coin_a.outpoint]);
            assert_eq!(coins.len(), 1);
            assert_eq!(coins[0].outpoint, coin_a.outpoint);
            let coins = conn.db_coins(&[coin_b.outpoint]);
            assert_eq!(coins.len(), 1);
            assert_eq!(coins[0].outpoint, coin_b.outpoint);
            let coins = conn.db_coins(&[coin_a.outpoint, coin_b.outpoint]);
            assert_eq!(coins.len(), 2);
            assert!(coins.iter().any(|c| c.outpoint == coin_a.outpoint));
            assert!(coins.iter().any(|c| c.outpoint == coin_b.outpoint));

            // They are both unconfirmed.
            assert_eq!(conn.coins(&[CoinStatus::Unconfirmed], &[]).len(), 2);
            assert!(conn
                .coins(
                    &[
                        CoinStatus::Confirmed,
                        CoinStatus::Spending,
                        CoinStatus::Spent
                    ],
                    &[]
                )
                .is_empty());

            // Now if we confirm one, it'll be marked as such.
            let height = 174500;
            let time = 174500;
            conn.confirm_coins(&[(coin_a.outpoint, height, time)]);
            let coins = conn.coins(&[], &[]);
            assert_eq!(coins[0].block_info, Some(DbBlockInfo { height, time }));
            assert!(coins[1].block_info.is_none());

            // Now if we spend one, it'll be marked as such.
            conn.spend_coins(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
            )]);
            let coin = conn
                .coins(&[], &[coin_a.outpoint])
                .into_iter()
                .next()
                .unwrap();
            assert!(coin.spend_txid.is_some());

            // We can unspend it, if the spend transaction gets double spent.
            conn.unspend_coins(&[coin_a.outpoint]);
            let coin = conn
                .coins(&[], &[coin_a.outpoint])
                .into_iter()
                .next()
                .unwrap();
            assert!(coin.spend_txid.is_none());

            // Spend it back. We will see it as 'spending'
            conn.spend_coins(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
            )]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .list_spending_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_a.outpoint));

            // The first one is spending, not the second one.
            assert_eq!(
                conn.coins(&[CoinStatus::Spending], &[])[0].outpoint,
                coin_a.outpoint
            );
            assert_eq!(
                conn.coins(&[CoinStatus::Unconfirmed], &[])[0].outpoint,
                coin_b.outpoint
            );

            // Now if we confirm the spend.
            let height = 128_097;
            let time = 3_000_000;
            conn.confirm_spend(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
                height,
                time,
            )]);
            // the coin is not in a spending state.
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .list_spending_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.is_empty());

            // Both are still in DB
            let coins = conn.db_coins(&[coin_a.outpoint, coin_b.outpoint]);
            assert_eq!(coins.len(), 2);

            // The confirmed one contains the right time and block height
            let coin = conn.db_coins(&[coin_a.outpoint]).pop().unwrap();
            assert!(coin.spend_block.is_some());
            assert_eq!(coin.spend_block.as_ref().unwrap().time, time);
            assert_eq!(coin.spend_block.unwrap().height, height);

            // If we unspend it all spend info will be wiped.
            conn.unspend_coins(&[coin_a.outpoint]);
            let coin = conn
                .coins(&[], &[coin_a.outpoint])
                .into_iter()
                .next()
                .unwrap();
            assert!(coin.spend_txid.is_none());
            assert!(coin.spend_block.is_none());

            // Add an immature coin. As all coins it's first registered as unconfirmed (even though
            // it's not).
            let coin_imma = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571937a:42",
                )
                .unwrap(),
                is_immature: true,
                block_info: None,
                amount: bitcoin::Amount::from_sat(424242),
                derivation_index: bip32::ChildNumber::from_normal_idx(4103).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_imma]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .coins(&[], &[])
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_imma.outpoint));
            let coin = conn.db_coins(&[coin_imma.outpoint]).pop().unwrap();
            assert!(coin.is_immature && !coin.is_change);

            // Confirming an immature coin marks it as mature.
            let (height, time) = (424242, 424241);
            conn.confirm_coins(&[(coin_imma.outpoint, height, time)]);
            let coin = conn.db_coins(&[coin_imma.outpoint]).pop().unwrap();
            assert!(!coin.is_immature);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn sqlite_addresses_cache() {
        let (tmp_dir, options, secp, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // There is the index for the first index
            let addr = options
                .main_descriptor
                .receive_descriptor()
                .derive(0.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 0.into());

            // And also for the change address
            let addr = options
                .main_descriptor
                .change_descriptor()
                .derive(0.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 0.into());

            // There is the index for the 199th index (look-ahead limit)
            let addr = options
                .main_descriptor
                .receive_descriptor()
                .derive(199.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 199.into());

            // And not for the 200th one.
            let addr = options
                .main_descriptor
                .receive_descriptor()
                .derive(200.into(), &secp)
                .address(options.bitcoind_network);
            assert!(conn.db_address(&addr).is_none());

            // But if we increment the deposit derivation index, the 200th one will be there.
            conn.set_derivation_index(1.into(), false, &secp);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 200.into());

            // It will also be there for the change descriptor.
            let addr = options
                .main_descriptor
                .change_descriptor()
                .derive(200.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 200.into());

            // But not for the 201th.
            let addr = options
                .main_descriptor
                .change_descriptor()
                .derive(201.into(), &secp)
                .address(options.bitcoind_network);
            assert!(conn.db_address(&addr).is_none());

            // If we increment the *change* derivation index to 1, it will still not be there.
            conn.set_derivation_index(1.into(), true, &secp);
            assert!(conn.db_address(&addr).is_none());

            // But incrementing it once again it will be there for both change and receive.
            conn.set_derivation_index(2.into(), true, &secp);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 201.into());
            let addr = options
                .main_descriptor
                .receive_descriptor()
                .derive(201.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 201.into());

            // Now setting it to a much higher will fill all the addresses within the gap
            conn.set_derivation_index(52.into(), true, &secp);
            for index in 2..52 {
                let look_ahead_index = 200 + index;
                let addr = options
                    .main_descriptor
                    .receive_descriptor()
                    .derive(look_ahead_index.into(), &secp)
                    .address(options.bitcoind_network);
                let db_addr = conn.db_address(&addr).unwrap();
                assert_eq!(db_addr.derivation_index, look_ahead_index.into());
            }

            // Suppose the latest change derivation index was set to 52 above by the commands
            // thread. Suppose concurrently the Bitcoin poller thread queried the DB for the
            // latest derivation just before it happened, got 2 as a response, and then increased
            // the derivation index to -say- 7 after noticing a new change output paying to the
            // address at derivation index 6. It's absolutely possible and the only way to prevent
            // this is to make sure *within* the atomic DB transaction that we will never decrease
            // the derivation index. Make sure we actually perform this check (note it would only
            // crash during the second call).
            conn.set_derivation_index(7.into(), true, &secp);
            conn.set_derivation_index(8.into(), true, &secp);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn sqlite_tip_rollback() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            let old_tip = BlockChainTip {
                hash: bitcoin::BlockHash::from_str(
                    "00000000000000000004f43b5e743757939082170673d27a5a5130e0eb238832",
                )
                .unwrap(),
                height: 200_000,
            };
            conn.update_tip(&old_tip);

            // 5 coins:
            // - One unconfirmed
            // - One confirmed before the rollback height
            // - One confirmed before the rollback height but spent after
            // - One confirmed after the rollback height
            // - One spent after the rollback height
            // TODO: immature deposits
            let coins = [
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "c449539458c60bee6c0d8905ba1dadb20b9187b82045d306a408b894cea492b0:2",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_095,
                        time: 1_111_899,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(100).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "f0801fd9ca8bca0624c230ab422b2e2c4c8dc995e4e1dbc6412510959cce1e4f:3",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_099,
                        time: 1_121_899,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(1000).unwrap(),
                    is_change: false,
                    spend_txid: Some(
                        bitcoin::Txid::from_str(
                            "0c62a990d20d54429e70859292e82374ba6b1b951a3ab60f26bb65fee5724ff7",
                        )
                        .unwrap(),
                    ),
                    spend_block: Some(BlockInfo {
                        height: 101_199,
                        time: 1_231_678,
                    }),
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "19f56e65069f0a7a3bfb00c6a7085cc0669e03e91befeca1ee9891c9e737b2fb:4",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_100,
                        time: 1_131_899,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(10000).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "ed6c8f1af9325f84de521e785e7ddfd33dc28c9ada4d687dcd3850100bde54e9:5",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_102,
                        time: 1_134_899,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(100000).unwrap(),
                    is_change: false,
                    spend_txid: Some(
                        bitcoin::Txid::from_str(
                            "7477017f992cdc7ba08acafb77cb3b5bc0f42ac340d3e1e1da0785bdda20d5f6",
                        )
                        .unwrap(),
                    ),
                    spend_block: Some(BlockInfo {
                        height: 101_105,
                        time: 1_201_678,
                    }),
                },
            ];
            conn.new_unspent_coins(&coins);
            conn.confirm_coins(
                &coins
                    .iter()
                    .filter_map(|c| c.block_info.map(|b| (c.outpoint, b.height, b.time)))
                    .collect::<Vec<_>>(),
            );
            conn.confirm_spend(
                &coins
                    .iter()
                    .filter_map(|c| {
                        c.spend_block
                            .as_ref()
                            .map(|b| (c.outpoint, c.spend_txid.unwrap(), b.height, b.time))
                    })
                    .collect::<Vec<_>>(),
            );
            let mut db_coins = conn
                .db_coins(
                    &coins
                        .iter()
                        .map(|c| c.outpoint)
                        .collect::<Vec<bitcoin::OutPoint>>(),
                )
                .into_iter()
                .map(Coin::from)
                .collect::<Vec<_>>();
            db_coins.sort_by(|c1, c2| c1.outpoint.vout.cmp(&c2.outpoint.vout));
            assert_eq!(&db_coins[..], &coins[..]);

            // Now that everything is settled, reorg to a previous height.
            let new_tip = BlockChainTip {
                hash: bitcoin::BlockHash::from_str(
                    "000000000000000000016440c591da27679abfa53ef44d45b016640dbd04e126",
                )
                .unwrap(),
                height: 101_099,
            };
            conn.rollback_tip(&new_tip);

            // The tip got updated
            let new_db_tip = conn.db_tip();
            assert_eq!(new_db_tip.block_height.unwrap(), new_tip.height);
            assert_eq!(new_db_tip.block_hash.unwrap(), new_tip.hash);

            // And so were the coins
            let db_coins = conn
                .db_coins(
                    &coins
                        .iter()
                        .map(|c| c.outpoint)
                        .collect::<Vec<bitcoin::OutPoint>>(),
                )
                .into_iter()
                .map(|c| (c.outpoint, Coin::from(c)))
                .collect::<HashMap<_, _>>();
            // The first coin is unchanged
            assert_eq!(db_coins[&coins[0].outpoint], coins[0]);
            // Same for the second one
            assert_eq!(db_coins[&coins[1].outpoint], coins[1]);
            // The third one got its spend confirmation info wiped, but only that
            let mut coin = coins[2];
            coin.spend_block = None;
            assert_eq!(db_coins[&coins[2].outpoint], coin);
            // The fourth one got its own confirmation info wiped
            let mut coin = coins[3];
            coin.block_info = None;
            assert_eq!(db_coins[&coins[3].outpoint], coin);
            // The fourth one got both is own confirmation and spend confirmation info wiped
            let mut coin = coins[4];
            coin.block_info = None;
            coin.spend_block = None;
            assert_eq!(db_coins[&coins[4].outpoint], coin);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn db_rescan() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // At first no rescan is ongoing
            let dummy_timestamp = 1_001;
            let db_wallet = conn.db_wallet();
            assert!(db_wallet.rescan_timestamp.is_none());
            assert!(db_wallet.timestamp > dummy_timestamp);

            // But if we set one there'll be
            conn.set_wallet_rescan_timestamp(dummy_timestamp);
            assert_eq!(conn.db_wallet().rescan_timestamp, Some(dummy_timestamp));

            // Once it's done the rescan timestamp will be erased, and the
            // wallet timestamp will be set to the dummy timestamp since it's
            // lower.
            conn.complete_wallet_rescan();
            let db_wallet = conn.db_wallet();
            assert!(db_wallet.rescan_timestamp.is_none());
            assert_eq!(db_wallet.timestamp, dummy_timestamp);

            // If we rescan from a later timestamp, we'll keep the existing
            // wallet timestamp afterward.
            conn.set_wallet_rescan_timestamp(dummy_timestamp + 1);
            assert_eq!(conn.db_wallet().rescan_timestamp, Some(dummy_timestamp + 1));
            conn.complete_wallet_rescan();
            let db_wallet = conn.db_wallet();
            assert!(db_wallet.rescan_timestamp.is_none());
            assert_eq!(db_wallet.timestamp, dummy_timestamp);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn sqlite_list_txids() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            let coins = [
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "c449539458c60bee6c0d8905ba1dadb20b9187b82045d306a408b894cea492b0:2",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_095,
                        time: 1_121_000,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(100).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "f0801fd9ca8bca0624c230ab422b2e2c4c8dc995e4e1dbc6412510959cce1e4f:3",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_099,
                        time: 1_122_000,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(1000).unwrap(),
                    is_change: false,
                    spend_txid: Some(
                        bitcoin::Txid::from_str(
                            "0c62a990d20d54429e70859292e82374ba6b1b951a3ab60f26bb65fee5724ff7",
                        )
                        .unwrap(),
                    ),
                    spend_block: Some(BlockInfo {
                        height: 101_199,
                        time: 1_123_000,
                    }),
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "19f56e65069f0a7a3bfb00c6a7085cc0669e03e91befeca1ee9891c9e737b2fb:4",
                    )
                    .unwrap(),
                    is_immature: true,
                    block_info: Some(BlockInfo {
                        height: 101_100,
                        time: 1_124_000,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(10000).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                },
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "ed6c8f1af9325f84de521e785e7ddfd33dc28c9ada4d687dcd3850100bde54e9:5",
                    )
                    .unwrap(),
                    is_immature: false,
                    block_info: Some(BlockInfo {
                        height: 101_102,
                        time: 1_125_000,
                    }),
                    amount: bitcoin::Amount::from_sat(98765),
                    derivation_index: bip32::ChildNumber::from_normal_idx(100000).unwrap(),
                    is_change: false,
                    spend_txid: Some(
                        bitcoin::Txid::from_str(
                            "7477017f992cdc7ba08acafb77cb3b5bc0f42ac340d3e1e1da0785bdda20d5f6",
                        )
                        .unwrap(),
                    ),
                    spend_block: Some(BlockInfo {
                        height: 101_105,
                        time: 1_126_000,
                    }),
                },
            ];
            conn.new_unspent_coins(&coins);
            conn.confirm_coins(
                &coins
                    .iter()
                    .filter_map(|c| c.block_info.map(|b| (c.outpoint, b.height, b.time)))
                    .collect::<Vec<_>>(),
            );
            conn.confirm_spend(
                &coins
                    .iter()
                    .filter_map(|c| {
                        c.spend_block
                            .as_ref()
                            .map(|b| (c.outpoint, c.spend_txid.unwrap(), b.height, b.time))
                    })
                    .collect::<Vec<_>>(),
            );

            let db_txids = conn.db_list_txids(1_123_000, 1_127_000, 10);
            assert_eq!(
                &db_txids[..],
                &[
                    bitcoin::Txid::from_str(
                        "7477017f992cdc7ba08acafb77cb3b5bc0f42ac340d3e1e1da0785bdda20d5f6"
                    )
                    .unwrap(),
                    bitcoin::Txid::from_str(
                        "ed6c8f1af9325f84de521e785e7ddfd33dc28c9ada4d687dcd3850100bde54e9"
                    )
                    .unwrap(),
                    bitcoin::Txid::from_str(
                        "19f56e65069f0a7a3bfb00c6a7085cc0669e03e91befeca1ee9891c9e737b2fb"
                    )
                    .unwrap(),
                    bitcoin::Txid::from_str(
                        "0c62a990d20d54429e70859292e82374ba6b1b951a3ab60f26bb65fee5724ff7"
                    )
                    .unwrap()
                ]
            );

            let db_txids = conn.db_list_txids(1_123_000, 1_127_000, 2);
            assert_eq!(
                &db_txids[..],
                &[
                    bitcoin::Txid::from_str(
                        "7477017f992cdc7ba08acafb77cb3b5bc0f42ac340d3e1e1da0785bdda20d5f6"
                    )
                    .unwrap(),
                    bitcoin::Txid::from_str(
                        "ed6c8f1af9325f84de521e785e7ddfd33dc28c9ada4d687dcd3850100bde54e9"
                    )
                    .unwrap(),
                ]
            );
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn v0_to_v2_migration() {
        let secp = secp256k1::Secp256k1::verification_only();

        // Create a database with version 0, using the old schema.
        let tmp_dir = tmp_dir();
        eprintln!("{}", tmp_dir.as_path().to_string_lossy());
        fs::create_dir_all(&tmp_dir).unwrap();
        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("lianad_v0.sqlite3")]
            .iter()
            .collect();
        let mut options = dummy_options();
        options.schema = V0_SCHEMA;
        options.version = 0;
        create_fresh_db(&db_path, options, &secp).unwrap();

        // Two PSBTs we'll insert in the DB before and after the migration. Note they are random
        // PSBTs taken from the descriptor unit tests, it doesn't matter.
        let first_psbt = psbt_from_str("cHNidP8BAIkCAAAAAWi3OFgkj1CqCDT3Swm8kbxZS9lxz4L3i4W2v9KGC7nqAQAAAAD9////AkANAwAAAAAAIgAg27lNc1rog+dOq80ohRuds4Hgg/RcpxVun2XwgpuLSrFYMwwAAAAAACIAIDyWveqaElWmFGkTbFojg1zXWHODtiipSNjfgi2DqBy9AAAAAAABAOoCAAAAAAEBsRWl70USoAFFozxc86pC7Dovttdg4kvja//3WMEJskEBAAAAAP7///8CWKmCIk4GAAAWABRKBWYWkCNS46jgF0r69Ehdnq+7T0BCDwAAAAAAIgAgTt5fs+CiB+FRzNC8lHcgWLH205sNjz1pT59ghXlG5tQCRzBEAiBXK9MF8z3bX/VnY2aefgBBmiAHPL4tyDbUOe7+KpYA4AIgL5kU0DFG8szKd+szRzz/OTUWJ0tZqij41h2eU9rSe1IBIQNBB1hy+jKsg1TihMT0dXw7etpu9TkO3NuvhBDFJlBj1cP2AQABAStAQg8AAAAAACIAIE7eX7PgogfhUczQvJR3IFix9tObDY89aU+fYIV5RubUIgICSKJsNs0zFJN58yd2aYQ+C3vhMbi0x7k0FV3wBhR4THlIMEUCIQCPWWWOhs2lThxOq/G8X2fYBRvM9MXSm7qPH+dRVYQZEwIgfut2vx3RvwZWcgEj4ohQJD5lNJlwOkA4PAiN1fjx6dABIgID3mvj1zerZKohOVhKCiskYk+3qrCum6PIwDhQ16ePACpHMEQCICZNR+0/1hPkrDQwPFmg5VjUHkh6aK9cXUu3kPbM8hirAiAyE/5NUXKfmFKij30isuyysJbq8HrURjivd+S9vdRGKQEBBZNSIQJIomw2zTMUk3nzJ3ZphD4Le+ExuLTHuTQVXfAGFHhMeSEC9OfCXl+sJOrxUFLBuMV4ZUlJYjuzNGZSld5ioY14y8FSrnNkUSED3mvj1zerZKohOVhKCiskYk+3qrCum6PIwDhQ16ePACohA+ECH+HlR+8Sf3pumaXH3IwSsoqSLCH7H1THiBP93z3ZUq9SsmgiBgJIomw2zTMUk3nzJ3ZphD4Le+ExuLTHuTQVXfAGFHhMeRxjat8/MAAAgAEAAIAAAACAAgAAgAAAAAABAAAAIgYC9OfCXl+sJOrxUFLBuMV4ZUlJYjuzNGZSld5ioY14y8Ec/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAQAAACIGA95r49c3q2SqITlYSgorJGJPt6qwrpujyMA4UNenjwAqHGNq3z8wAACAAQAAgAEAAIACAACAAAAAAAEAAAAiBgPhAh/h5UfvEn96bpmlx9yMErKKkiwh+x9Ux4gT/d892Rz/1jyNMAAAgAEAAIABAACAAgAAgAAAAAABAAAAACICAlBQ7gGocg7eF3sXrCio+zusAC9+xfoyIV95AeR69DWvHGNq3z8wAACAAQAAgAEAAIACAACAAAAAAAMAAAAiAgMvVy984eg8Kgvj058PBHetFayWbRGb7L0DMnS9KHSJzBxjat8/MAAAgAEAAIAAAACAAgAAgAAAAAADAAAAIgIDSRIG1dn6njdjsDXenHa2lUvQHWGPLKBVrSzbQOhiIxgc/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAwAAACICA0/epE59sVEj7Et0I4R9qJQNuX23RNvDZKCRL7eUps9FHP/WPI0wAACAAQAAgAEAAIACAACAAAAAAAMAAAAAIgICgldCOK6iHscv//2NipgaMABLV5TICU/zlP7HlQmlg08cY2rfPzAAAIABAACAAQAAgAIAAIABAAAAAQAAACICApb0p9rfpJshB3J186PGWrvzQdixcwQZWmebOUMdkquZHP/WPI0wAACAAQAAgAAAAIACAACAAQAAAAEAAAAiAgLY5q+unoDxC/HI5BaNiPq12ei1REZIcUAN304JfKXUwxz/1jyNMAAAgAEAAIABAACAAgAAgAEAAAABAAAAIgIDg6cUVCJB79cMcofiURHojxFARWyS4YEhJNRixuOZZRgcY2rfPzAAAIABAACAAAAAgAIAAIABAAAAAQAAAAA=");
        let second_psbt = psbt_from_str("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");

        // The helper that was used to store Spend transaction in previous versions of the software
        // when there was no associated timestamp.
        fn store_spend_old(conn: &mut rusqlite::Connection, psbt: &Psbt) {
            let txid = &psbt.unsigned_tx.txid()[..].to_vec();

            db_exec(conn, |db_tx| {
                db_tx.execute(
                    "INSERT into spend_transactions (psbt, txid) VALUES (?1, ?2) \
                     ON CONFLICT DO UPDATE SET psbt=excluded.psbt",
                    rusqlite::params![psbt.serialize(), txid],
                )?;
                Ok(())
            })
            .expect("Db must not fail");
        }

        // Store a PSBT before the migration.
        {
            let mut conn = rusqlite::Connection::open(&db_path).unwrap();
            store_spend_old(&mut conn, &first_psbt);
        }

        // The helper that was used to store coins in previous versions of the software, stripped
        // down to a single coin.
        fn store_coin_old(
            conn: &mut rusqlite::Connection,
            outpoint: &bitcoin::OutPoint,
            amount: bitcoin::Amount,
            derivation_index: bip32::ChildNumber,
            is_change: bool,
        ) {
            db_exec(conn, |db_tx| {
                    let deriv_index: u32 = derivation_index.into();
                    db_tx.execute(
                        "INSERT INTO coins (wallet_id, txid, vout, amount_sat, derivation_index, is_change) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            WALLET_ID,
                            outpoint.txid[..].to_vec(),
                            outpoint.vout,
                            amount.to_sat(),
                            deriv_index,
                            is_change,
                        ],
                    )?;
                Ok(())
            })
            .expect("Database must be available")
        }

        // Store a couple coins before the migration.
        {
            let mut conn = rusqlite::Connection::open(&db_path).unwrap();
            store_coin_old(
                &mut conn,
                &bitcoin::OutPoint::from_str(
                    "ed6c8f1af9325f84de521e785e7ddfd33dc28c9ada4d687dcd3850100bde54e9:5",
                )
                .unwrap(),
                bitcoin::Amount::from_sat(14_000),
                24.into(),
                true,
            );
            store_coin_old(
                &mut conn,
                &bitcoin::OutPoint::from_str(
                    "81b2f327d4c1fd67afd039374f8798fd9ff37932c6f5c221c1c569350eac5ac8:2",
                )
                .unwrap(),
                bitcoin::Amount::from_sat(392_093_123),
                24_567.into(),
                false,
            );
        }

        // Migrate the DB.
        maybe_apply_migration(&db_path).unwrap();
        maybe_apply_migration(&db_path).unwrap(); // Migrating twice will be a no-op.
        let db = SqliteDb::new(db_path, None, &secp).unwrap();

        // We should now be able to insert another PSBT, to query both, and the first PSBT must
        // have no associated timestamp.
        {
            let mut conn = db.connection().unwrap();
            conn.store_spend(&second_psbt);
            let db_spends = conn.list_spend();
            let first_spend = db_spends
                .iter()
                .find(|db_spend| db_spend.psbt == first_psbt)
                .unwrap();
            assert!(first_spend.updated_at.is_none());
            let second_spend = db_spends
                .iter()
                .find(|db_spend| db_spend.psbt == second_psbt)
                .unwrap();
            assert!(second_spend.updated_at.is_some());
        }

        // We should now be able to store an immature coin, query all of them, and the first two
        // should not be immature.
        {
            let mut conn = db.connection().unwrap();
            conn.new_unspent_coins(&[Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                )
                .unwrap(),
                is_immature: true,
                block_info: None,
                amount: bitcoin::Amount::from_sat(98765),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            }]);
            let coins = conn.coins(&[], &[]);
            assert_eq!(coins.len(), 3);
            assert_eq!(coins.iter().filter(|c| !c.is_immature).count(), 2);
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn v0_to_v3_migration() {
        let secp = secp256k1::Secp256k1::verification_only();

        // Create a database with version 0, using the old schema.
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("lianad_v0.sqlite3")]
            .iter()
            .collect();
        let mut options = dummy_options();
        options.schema = V0_SCHEMA;
        options.version = 0;
        create_fresh_db(&db_path, options, &secp).unwrap();

        // SqliteDb new is doing the migration.
        let db = SqliteDb::new(db_path, None, &secp).unwrap();

        {
            let mut conn = db.connection().unwrap();
            let version = conn.db_version();
            assert_eq!(version, 3);

            let txid_str = "0c62a990d20d54429e70859292e82374ba6b1b951a3ab60f26bb65fee5724ff7";
            let txid = LabelItem::from_str(txid_str, bitcoin::Network::Bitcoin).unwrap();
            let mut txids_labels = HashMap::new();
            txids_labels.insert(txid.clone(), Some("hello".to_string()));
            conn.update_labels(&txids_labels);

            let mut items = HashSet::new();
            items.insert(txid);
            let db_labels = conn.db_labels(&items);
            assert_eq!(db_labels[0].value, "hello");
        }

        fs::remove_dir_all(tmp_dir).unwrap();
    }
}
