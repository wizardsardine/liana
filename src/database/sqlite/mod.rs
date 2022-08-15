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
            schema::{DbCoin, DbTip, DbWallet},
            utils::{create_fresh_db, db_exec, db_query},
        },
        Coin,
    },
};

use std::{convert::TryInto, fmt, io, path};

use miniscript::{
    bitcoin::{self, util::bip32},
    Descriptor, DescriptorPublicKey,
};

const DB_VERSION: i64 = 0;

#[derive(Debug)]
pub enum SqliteDbError {
    FileCreation(io::Error),
    FileNotFound(path::PathBuf),
    UnsupportedVersion(i64),
    InvalidNetwork(bitcoin::Network),
    DescriptorMismatch(Descriptor<DescriptorPublicKey>),
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
    pub main_descriptor: Descriptor<DescriptorPublicKey>,
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
    ) -> Result<SqliteDb, SqliteDbError> {
        // Create the database if needed, and make sure the db file exists.
        if let Some(options) = fresh_options {
            create_fresh_db(&db_path, options)?;
            log::info!("Created a fresh database at {}.", db_path.display());
        }
        if !db_path.exists() {
            return Err(SqliteDbError::FileNotFound(db_path.to_path_buf()));
        }

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
        main_descriptor: &Descriptor<DescriptorPublicKey>,
    ) -> Result<(), SqliteDbError> {
        let mut conn = self.connection()?;

        // Check if there database isn't from the future.
        // NOTE: we'll do migration there eventually. Until then be strict on the check.
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
            return Err(SqliteDbError::DescriptorMismatch(db_wallet.main_descriptor));
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
        db_query(
            &mut self.conn,
            "SELECT version FROM version",
            rusqlite::params![],
            |row| {
                let version: i64 = row.get(0)?;
                Ok(version)
            },
        )
        .expect("db must not fail")
        .pop()
        .expect("There is always a row in the version table")
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

    /// Update the deposit derivation index.
    pub fn update_derivation_index(&mut self, index: bip32::ChildNumber) {
        let new_index: u32 = index.into();
        db_exec(&mut self.conn, |db_tx| {
            // NOTE: should be updated if we ever have multi-wallet support
            db_tx
                .execute(
                    "UPDATE wallets SET deposit_derivation_index = (?1)",
                    rusqlite::params![new_index],
                )
                .map(|_| ())
        })
        .expect("Database must be available")
    }

    /// Get all UTxOs.
    pub fn unspent_coins(&mut self) -> Vec<DbCoin> {
        db_query(
            &mut self.conn,
            "SELECT * FROM coins WHERE spend_txid is NULL",
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
                    "INSERT INTO coins (wallet_id, txid, vout, amount_sat, derivation_index) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        WALLET_ID,
                        coin.outpoint.txid.to_vec(),
                        coin.outpoint.vout,
                        coin.amount.as_sat(),
                        deriv_index,
                    ],
                )?;
            }
            Ok(())
        })
        .expect("Database must be available")
    }

    /// Mark a set of coins as confirmed.
    pub fn confirm_coins<'a>(
        &mut self,
        outpoints: impl IntoIterator<Item = &'a (bitcoin::OutPoint, i32)>,
    ) {
        db_exec(&mut self.conn, |db_tx| {
            for (outpoint, height) in outpoints {
                db_tx.execute(
                    "UPDATE coins SET blockheight = ?1 WHERE txid = ?2 AND vout = ?3",
                    rusqlite::params![height, outpoint.txid.to_vec(), outpoint.vout,],
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::*;
    use std::{collections::HashSet, fs, path, str::FromStr};

    use bitcoin::hashes::Hash;

    fn dummy_options() -> FreshDbOptions {
        let desc_str = "wsh(andor(pk(03b506a1dbe57b4bf48c95e0c7d417b87dd3b4349d290d2e7e9ba72c912652d80a),older(10000),pk(0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce)))";
        let main_descriptor = Descriptor::<DescriptorPublicKey>::from_str(desc_str).unwrap();
        FreshDbOptions {
            bitcoind_network: bitcoin::Network::Bitcoin,
            main_descriptor,
        }
    }

    fn dummy_db() -> (path::PathBuf, FreshDbOptions, SqliteDb) {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("minisafed.sqlite3")]
            .iter()
            .collect();
        let options = dummy_options();
        let db = SqliteDb::new(db_path.clone(), Some(options.clone())).unwrap();

        (tmp_dir, options, db)
    }

    #[test]
    fn db_startup_sanity_checks() {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("minisafed.sqlite3")]
            .iter()
            .collect();
        assert!(SqliteDb::new(db_path.clone(), None)
            .unwrap_err()
            .to_string()
            .contains("database file not found"));

        let options = dummy_options();

        let db = SqliteDb::new(db_path.clone(), Some(options.clone())).unwrap();
        db.sanity_check(bitcoin::Network::Testnet, &options.main_descriptor)
            .unwrap_err()
            .to_string()
            .contains("Database was created for network");
        fs::remove_file(&db_path).unwrap();
        let other_desc_str = "wsh(andor(pk(037a27a76ebf33594c785e4fa41607860a960bb5aa3039654297b05bff57e4f9a9),older(10000),pk(0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce)))";
        let other_desc = Descriptor::<DescriptorPublicKey>::from_str(other_desc_str).unwrap();
        let db = SqliteDb::new(db_path.clone(), Some(options.clone())).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &other_desc)
            .unwrap_err()
            .to_string()
            .contains("Database descriptor mismatch");
        fs::remove_file(&db_path).unwrap();
        // TODO: version check

        let db = SqliteDb::new(db_path.clone(), Some(options.clone())).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &options.main_descriptor)
            .unwrap();
        let db = SqliteDb::new(db_path.clone(), None).unwrap();
        db.sanity_check(bitcoin::Network::Bitcoin, &options.main_descriptor)
            .unwrap();

        fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn db_tip_update() {
        let (tmp_dir, options, db) = dummy_db();

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

        fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn db_coins_update() {
        let (tmp_dir, _, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // Necessarily empty at first.
            assert!(conn.unspent_coins().is_empty());

            // Add one, we'll get it.
            let coin_a = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c:1",
                )
                .unwrap(),
                block_height: None,
                amount: bitcoin::Amount::from_sat(98765),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                spend_txid: None,
            };
            conn.new_unspent_coins(&[coin_a.clone()]); // On 1.48, arrays aren't IntoIterator
            assert_eq!(conn.unspent_coins()[0].outpoint, coin_a.outpoint);

            // Add a second one, we'll get both.
            let coin_b = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571936f:12",
                )
                .unwrap(),
                block_height: None,
                amount: bitcoin::Amount::from_sat(1111),
                derivation_index: bip32::ChildNumber::from_normal_idx(103).unwrap(),
                spend_txid: None,
            };
            conn.new_unspent_coins(&[coin_b.clone()]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .unspent_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_a.outpoint));
            assert!(outpoints.contains(&coin_b.outpoint));

            // Now if we confirm one, it'll be marked as such.
            let height = 174500;
            conn.confirm_coins(&[(coin_a.outpoint, height)]);
            let coins = conn.unspent_coins();
            assert_eq!(coins[0].block_height, Some(height));
            assert!(coins[1].block_height.is_none());

            // Now if we spend one, we'll only get the other one.
            conn.spend_coins(&[(
                coin_a.outpoint,
                bitcoin::Txid::from_slice(&[0; 32][..]).unwrap(),
            )]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .unspent_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(!outpoints.contains(&coin_a.outpoint));
            assert!(outpoints.contains(&coin_b.outpoint));
        }

        fs::remove_dir_all(&tmp_dir).unwrap();
    }
}
