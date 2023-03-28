use crate::database::sqlite::{utils::db_exec, FreshDbOptions, SqliteDbError};

use std::path;

use miniscript::bitcoin::secp256k1;

pub const DB_VERSION: i64 = 0;

pub trait Migration {
    fn version(&self) -> i64;
    fn apply(self) -> Result<(), SqliteDbError>;
}

pub const MIGRATION_0_VERSION: i64 = 0;
pub const MIGRATION_0_LOOK_AHEAD_LIMIT: u32 = 200;
pub const MIGRATION_0: &str = "\
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

/// MigrationV0 is never used because either a fresh database is created with
/// `utils::create_fresh_db` using the current up to date SCHEMA, either the
/// database is already created and migrations with version superior or equal to 1 must be applied.
/// This code is only used in order to test and verify that the ordered application of all migrations
/// is equal to the current up to date database state created by the SCHEMA and the
/// `utils::create_fresh_db` function.
pub struct MigrationV0<'a> {
    db_path: &'a path::Path,
    options: FreshDbOptions,
    secp: &'a secp256k1::Secp256k1<secp256k1::VerifyOnly>,
}

impl<'a> MigrationV0<'a> {
    #[allow(dead_code)]
    pub fn new(
        db_path: &'a path::Path,
        options: FreshDbOptions,
        secp: &'a secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) -> Self {
        Self {
            db_path,
            options,
            secp,
        }
    }
}

impl<'a> Migration for MigrationV0<'a> {
    fn version(&self) -> i64 {
        MIGRATION_0_VERSION
    }
    fn apply(self) -> Result<(), SqliteDbError> {
        // Fill the initial addresses. On a fresh database, the deposit_derivation_index is
        // necessarily 0.
        let mut query = String::with_capacity(100 * MIGRATION_0_LOOK_AHEAD_LIMIT as usize);
        for index in 0..MIGRATION_0_LOOK_AHEAD_LIMIT {
            let receive_address = self
                .options
                .main_descriptor
                .receive_descriptor()
                .derive(index.into(), self.secp)
                .address(self.options.bitcoind_network);
            let change_address = self
                .options
                .main_descriptor
                .change_descriptor()
                .derive(index.into(), self.secp)
                .address(self.options.bitcoind_network);
            query += &format!(
            "INSERT INTO addresses (receive_address, change_address, derivation_index) VALUES (\"{}\", \"{}\", {});\n",
            receive_address, change_address, index
        );
        }

        let mut conn = rusqlite::Connection::open(self.db_path)?;
        db_exec(&mut conn, |tx| {
            tx.execute_batch(MIGRATION_0)?;
            tx.execute(
                "INSERT INTO version (version) VALUES (?1)",
                rusqlite::params![MIGRATION_0_VERSION],
            )?;
            tx.execute(
                "INSERT INTO tip (network, blockheight, blockhash) VALUES (?1, NULL, NULL)",
                rusqlite::params![self.options.bitcoind_network.to_string()],
            )?;
            tx.execute(
            "INSERT INTO wallets (timestamp, main_descriptor, deposit_derivation_index, change_derivation_index) \
                     VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![self.options.timestamp, self.options.main_descriptor.to_string(), 0, 0],
        )?;
            tx.execute_batch(&query)?;

            Ok(())
        })?;

        Ok(())
    }
}
