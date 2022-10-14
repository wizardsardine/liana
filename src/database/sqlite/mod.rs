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
            utils::{create_fresh_db, db_exec, db_query, db_tx_query, LOOK_AHEAD_LIMIT},
        },
        Coin,
    },
    descriptors::InheritanceDescriptor,
};

use std::{convert::TryInto, fmt, io, path};

use miniscript::bitcoin::{
    self, consensus::encode, hashes::hex::ToHex, secp256k1,
    util::psbt::PartiallySignedTransaction as Psbt,
};

const DB_VERSION: i64 = 0;

#[derive(Debug)]
pub enum SqliteDbError {
    FileCreation(io::Error),
    FileNotFound(path::PathBuf),
    UnsupportedVersion(i64),
    InvalidNetwork(bitcoin::Network),
    DescriptorMismatch(Box<InheritanceDescriptor>),
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
    pub main_descriptor: InheritanceDescriptor,
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
        main_descriptor: &InheritanceDescriptor,
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

    pub fn increment_derivation_index(
        &mut self,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        let network = self.db_tip().network;

        db_exec(&mut self.conn, |db_tx| {
            let db_wallet: DbWallet =
                db_tx_query(db_tx, "SELECT * FROM wallets", rusqlite::params![], |row| {
                    row.try_into()
                })
                .expect("Db must not fail")
                .pop()
                .expect("There is always a row in the wallet table");
            let next_index: u32 = db_wallet
                .deposit_derivation_index
                .increment()
                .expect("Must not get in hardened territory")
                .into();
            // NOTE: should be updated if we ever have multi-wallet support
            db_tx.execute(
                "UPDATE wallets SET deposit_derivation_index = (?1)",
                rusqlite::params![next_index],
            )?;

            // Update the address to derivation index mapping.
            // TODO: have this as a helper in descriptors.rs
            let next_la_index = next_index + LOOK_AHEAD_LIMIT - 1;
            let next_la_address = db_wallet
                .main_descriptor
                .derive(next_la_index.into(), secp)
                .address(network);
            db_tx
                .execute(
                    "INSERT INTO addresses (address, derivation_index) VALUES (?1, ?2)",
                    rusqlite::params![next_la_address.to_string(), next_la_index],
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
            "SELECT * FROM addresses WHERE address = ?1",
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
        let txid = psbt.global.unsigned_tx.txid().to_vec();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::*;
    use std::{collections::HashSet, fs, path, str::FromStr};

    use bitcoin::{hashes::Hash, util::bip32};

    fn dummy_options() -> FreshDbOptions {
        let desc_str = "wsh(andor(pk(tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/*),older(10000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))#y5wcna2d";
        let main_descriptor = InheritanceDescriptor::from_str(desc_str).unwrap();
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

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("minisafed.sqlite3")]
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

        let db_path: path::PathBuf = [tmp_dir.as_path(), path::Path::new("minisafed.sqlite3")]
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
        let other_desc_str = "wsh(andor(pk(tpubDExU4YLJkyQ9RRbVScQq2brFxWWha7WmAUByPWyaWYwmcTv3Shx8aHp6mVwuE5n4TeM4z5DTWGf2YhNPmXtfvyr8cUDVvA3txdrFnFgNdF7/*),older(10000),pk(tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/*)))";
        let other_desc = InheritanceDescriptor::from_str(other_desc_str).unwrap();
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

        fs::remove_dir_all(&tmp_dir).unwrap();
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

        fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn db_coins_update() {
        let (tmp_dir, _, _, db) = dummy_db();

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
                block_time: None,
                amount: bitcoin::Amount::from_sat(98765),
                derivation_index: bip32::ChildNumber::from_normal_idx(10).unwrap(),
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_a.clone()]); // On 1.48, arrays aren't IntoIterator
            assert_eq!(conn.unspent_coins()[0].outpoint, coin_a.outpoint);

            // We can query it by its outpoint
            let coins = conn.db_coins(&[coin_a.outpoint]);
            assert_eq!(coins.len(), 1);
            assert_eq!(coins[0].outpoint, coin_a.outpoint);

            // Add a second one, we'll get both.
            let coin_b = Coin {
                outpoint: bitcoin::OutPoint::from_str(
                    "61db3e276b095e5b05f1849dd6bfffb4e7e5ec1c4a4210099b98fce01571936f:12",
                )
                .unwrap(),
                block_height: None,
                block_time: None,
                amount: bitcoin::Amount::from_sat(1111),
                derivation_index: bip32::ChildNumber::from_normal_idx(103).unwrap(),
                spend_txid: None,
                spend_block: None,
            };
            conn.new_unspent_coins(&[coin_b.clone()]);
            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .unspent_coins()
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

            // Now if we confirm one, it'll be marked as such.
            let height = 174500;
            let time = 174500;
            conn.confirm_coins(&[(coin_a.outpoint, height, time)]);
            let coins = conn.unspent_coins();
            assert_eq!(coins[0].block_height, Some(height));
            assert_eq!(coins[0].block_time, Some(time));
            assert!(coins[1].block_height.is_none());
            assert!(coins[1].block_time.is_none());

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

            let outpoints: HashSet<bitcoin::OutPoint> = conn
                .list_spending_coins()
                .into_iter()
                .map(|c| c.outpoint)
                .collect();
            assert!(outpoints.contains(&coin_a.outpoint));

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

        fs::remove_dir_all(&tmp_dir).unwrap();
    }

    #[test]
    fn sqlite_addresses_cache() {
        let (tmp_dir, options, secp, db) = dummy_db();

        {
            let mut conn = db.connection().unwrap();

            // There is the index for the first index
            let addr = options
                .main_descriptor
                .derive(0.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 0.into());

            // There is the index for the 199th index (look-ahead limit)
            let addr = options
                .main_descriptor
                .derive(199.into(), &secp)
                .address(options.bitcoind_network);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 199.into());

            // And not for the 200th one.
            let addr = options
                .main_descriptor
                .derive(200.into(), &secp)
                .address(options.bitcoind_network);
            assert!(conn.db_address(&addr).is_none());

            // But if we increment the deposit derivation index, the 200th one will be there.
            conn.increment_derivation_index(&secp);
            let db_addr = conn.db_address(&addr).unwrap();
            assert_eq!(db_addr.derivation_index, 200.into());
        }

        fs::remove_dir_all(&tmp_dir).unwrap();
    }
}
