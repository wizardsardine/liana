///! Implementation of the database interface using SQLite.
///!
///! We use a bundled SQLite that is compiled with SQLITE_THREADSAFE. Sqlite.org states:
///! > Multi-thread. In this mode, SQLite can be safely used by multiple threads provided that
///! > no single database connection is used simultaneously in two or more threads.
///!
///! We leverage SQLite's `unlock_notify` feature to synchronize writes accross connection. More
///! about it at https://sqlite.org/unlock_notify.html.
pub mod schema;
mod utils;

use crate::{
    bitcoin::BlockChainTip,
    database::{
        sqlite::{
            schema::{DbAddress, DbCoin, DbSpendTransaction, DbTip, DbWallet},
            utils::{
                create_fresh_db, db_exec, db_query, db_tx_query, db_version, maybe_apply_migration,
                LOOK_AHEAD_LIMIT,
            },
        },
        Coin, CoinType,
    },
    descriptors::LianaDescriptor,
};

use std::{cmp, convert::TryInto, fmt, io, path};

use miniscript::bitcoin::{
    self,
    consensus::encode,
    hashes::hex::ToHex,
    secp256k1,
    util::{bip32, psbt::PartiallySignedTransaction as Psbt},
};

const DB_VERSION: i64 = 1;

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

#[derive(Debug, Clone)]
pub struct FreshDbOptions {
    pub bitcoind_network: bitcoin::Network,
    pub main_descriptor: LianaDescriptor,
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
                    rusqlite::params![tip.height, tip.hash.to_vec()],
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

    /// Get all the coins from DB.
    pub fn coins(&mut self, coin_type: CoinType) -> Vec<DbCoin> {
        db_query(
            &mut self.conn,
            match coin_type {
                CoinType::All => "SELECT * FROM coins",
                CoinType::Unspent => "SELECT * FROM coins WHERE spend_txid IS NULL",
                CoinType::Spent => "SELECT * FROM coins WHERE spend_txid IS NOT NULL",
            },
            rusqlite::params![],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
    }

    /// List coins that are being spent and whose spending transaction is still unconfirmed.
    pub fn list_spending_coins(&mut self) -> Vec<DbCoin> {
        db_query(
            &mut self.conn,
            "SELECT * FROM coins WHERE spend_txid IS NOT NULL AND spend_block_time IS NULL",
            rusqlite::params![],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
    }

    // FIXME: don't take the whole coin, we don't need it.
    /// Store new, unconfirmed and unspent, coins.
    /// Will panic if given a coin that is already in DB.
    pub fn new_unspent_coins<'a>(&mut self, coins: impl IntoIterator<Item = &'a Coin>) {
        db_exec(&mut self.conn, |db_tx| {
            for coin in coins {
                let deriv_index: u32 = coin.derivation_index.into();
                db_tx.execute(
                    "INSERT INTO coins (wallet_id, txid, vout, amount_sat, derivation_index, is_change) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        WALLET_ID,
                        coin.outpoint.txid.to_vec(),
                        coin.outpoint.vout,
                        coin.amount.to_sat(),
                        deriv_index,
                        coin.is_change,
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
                    rusqlite::params![outpoint.txid.to_vec(), outpoint.vout,],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as confirmed.
    pub fn confirm_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, i32, u32)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, height, time) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET blockheight = ?1, blocktime = ?2 WHERE txid = ?3 AND vout = ?4",
                    rusqlite::params![height, time, outpoint.txid.to_vec(), outpoint.vout,],
                )?;
            }

            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as spent.
    pub fn spend_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, bitcoin::Txid)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, spend_txid) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET spend_txid = ?1 WHERE txid = ?2 AND vout = ?3",
                    rusqlite::params![spend_txid.to_vec(), outpoint.txid.to_vec(), outpoint.vout,],
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
                        spend_txid.to_vec(),
                        height,
                        time,
                        outpoint.txid.to_vec(),
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
        // SELECT * FROM coins WHERE (txid, vout) IN ((txidA, voutA), (txidB, voutB));
        let mut query = "SELECT * FROM coins WHERE (txid, vout) IN (VALUES ".to_string();
        for (i, outpoint) in outpoints.iter().enumerate() {
            // NOTE: the txid is not stored as little-endian. Convert it to vec first.
            query += &format!(
                "(x'{}', {})",
                &outpoint.txid.to_vec().to_hex(),
                outpoint.vout
            );
            if i != outpoints.len() - 1 {
                query += ", ";
            }
        }
        query += ")";

        db_query(&mut self.conn, &query, rusqlite::params![], |row| {
            row.try_into()
        })
        .expect("Db must not fail")
    }

    pub fn db_spend(&mut self, txid: &bitcoin::Txid) -> Option<DbSpendTransaction> {
        db_query(
            &mut self.conn,
            "SELECT * FROM spend_transactions WHERE txid = ?1",
            rusqlite::params![txid.to_vec()],
            |row| row.try_into(),
        )
        .expect("Db must not fail")
        .pop()
    }

    /// Insert a new Spend transaction or replace an existing one.
    pub fn store_spend(&mut self, psbt: &Psbt) {
        let txid = psbt.unsigned_tx.txid().to_vec();
        let psbt = encode::serialize(psbt);

        db_exec(&mut self.conn, |db_tx| {
            db_tx.execute(
                "INSERT into spend_transactions (psbt, txid) VALUES (?1, ?2) \
                 ON CONFLICT DO UPDATE SET psbt=excluded.psbt",
                rusqlite::params![psbt, txid],
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
                rusqlite::params![txid.to_vec()],
            )?;
            Ok(())
        })
        .expect("Db must not fail");
    }

    /// Unconfirm all data that was marked as being confirmed *after* the given chain
    /// tip, and set it as our new best block seen.
    ///
    /// This includes:
    /// - Coins
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
                rusqlite::params![new_tip.height, new_tip.hash.to_vec()],
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

    use bitcoin::{hashes::Hash, util::bip32};

    fn dummy_options() -> FreshDbOptions {
        let desc_str = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs";
        let main_descriptor = LianaDescriptor::from_str(desc_str).unwrap();
        FreshDbOptions {
            bitcoind_network: bitcoin::Network::Bitcoin,
            main_descriptor,
        }
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
    fn db_coins_update() {
        let (tmp_dir, _, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // Necessarily empty at first.
            assert!(conn.coins(CoinType::All).is_empty());

            // Add one, we'll get it.
            let coin_a = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                )
                .unwrap(),
                block_info: None,
                amount: bitcoin::Amount::from_sat(98765),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_a]);
            assert_eq!(conn.coins(CoinType::All)[0].outpoint, coin_a.outpoint);

            // We can also remove it. Say the unconfirmed tx that created it got replaced.
            conn.remove_coins(&[coin_a.outpoint]);
            assert!(conn.coins(CoinType::All).is_empty());

            // Add it back for the rest of the test.
            conn.new_unspent_coins(&[coin_a]);

            // We can query it by its outpoint
            let coins = conn.db_coins(&[coin_a.outpoint]);
            assert_eq!(coins.len(), 1);
            assert_eq!(coins[0].outpoint, coin_a.outpoint);

            // It is unspent.
            assert_eq!(conn.coins(CoinType::Unspent)[0].outpoint, coin_a.outpoint);
            assert!(conn.coins(CoinType::Spent).is_empty());

            // Add a second one (this one is change), we'll get both.
            let coin_b = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571936f:12",
                )
                .unwrap(),
                block_info: None,
                amount: bitcoin::Amount::from_sat(1111),
                derivation_index: bip32::ChildNumber::from_normal_idx(103).unwrap(),
                is_change: true,
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_b]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .coins(CoinType::All)
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

            // They are both unspent
            assert_eq!(conn.coins(CoinType::Unspent).len(), 2);
            assert!(conn.coins(CoinType::Spent).is_empty());

            // Now if we confirm one, it'll be marked as such.
            let height = 174500;
            let time = 174500;
            conn.confirm_coins(&[(coin_a.outpoint, height, time)]);
            let coins = conn.coins(CoinType::All);
            assert_eq!(coins[0].block_info, Some(DbBlockInfo { height, time }));
            assert!(coins[1].block_info.is_none());

            // Now if we spend one, it'll be marked as such.
            conn.spend_coins(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
            )]);
            let coins_map: HashMap<bitcoin::OutPoint, DbCoin> = conn
                .coins(CoinType::All)
                .into_iter()
                .map(|c| (c.outpoint, c))
                .collect();
            assert!(coins_map
                .get(&coin_a.outpoint)
                .unwrap()
                .spend_txid
                .is_some());

            // We will see it as 'spending'
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .list_spending_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_a.outpoint));

            // The first one is spent, not the second one.
            assert_eq!(conn.coins(CoinType::Spent)[0].outpoint, coin_a.outpoint);
            assert_eq!(conn.coins(CoinType::Unspent)[0].outpoint, coin_b.outpoint);

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
            let coins = [
                Coin {
                    outpoint: bitcoin::OutPoint::from_str(
                        "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                    )
                    .unwrap(),
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
}
