use crate::database::sqlite::{
    schema::{DbWallet, SCHEMA},
    FreshDbOptions, SqliteDbError, DB_VERSION,
};

use std::{convert::TryInto, fs, path, time};

use miniscript::bitcoin::{self, secp256k1};

pub const LOOK_AHEAD_LIMIT: u32 = 200;

/// Perform a set of modifications to the database inside a single transaction
pub fn db_exec<F>(conn: &mut rusqlite::Connection, modifications: F) -> Result<(), rusqlite::Error>
where
    F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<()>,
{
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    modifications(&tx)?;
    tx.commit()
}

/// Internal helper for queries boilerplate
pub fn db_tx_query<P, F, T>(
    tx: &rusqlite::Transaction,
    stmt_str: &str,
    params: P,
    f: F,
) -> Result<Vec<T>, rusqlite::Error>
where
    P: IntoIterator + rusqlite::Params,
    P::Item: rusqlite::ToSql,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    tx.prepare(stmt_str)?
        .query_map(params, f)?
        .collect::<rusqlite::Result<Vec<T>>>()
}

/// Internal helper for queries boilerplate
pub fn db_query<P, F, T>(
    conn: &mut rusqlite::Connection,
    stmt_str: &str,
    params: P,
    f: F,
) -> Result<Vec<T>, rusqlite::Error>
where
    P: IntoIterator + rusqlite::Params,
    P::Item: rusqlite::ToSql,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    conn.prepare(stmt_str)?
        .query_map(params, f)?
        .collect::<rusqlite::Result<Vec<T>>>()
}

// Sqlite supports up to i64, thus rusqlite prevents us from inserting u64's.
// We use this to panic rather than inserting a truncated integer into the database (as we'd have
// done by using `n as u32`).
fn timestamp_to_u32(n: u64) -> u32 {
    n.try_into()
        .expect("Is this the year 2106 yet? Misconfigured system clock.")
}

// Create the db file with RW permissions only for the user
pub fn create_db_file(db_path: &path::Path) -> Result<(), std::io::Error> {
    let mut options = fs::OpenOptions::new();
    let options = options.read(true).write(true).create_new(true);

    #[cfg(unix)]
    return {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600).open(db_path)?;
        Ok(())
    };

    #[cfg(not(unix))]
    return {
        // TODO: permissions for Windows...
        options.open(db_path)?;
        Ok(())
    };
}

pub fn create_fresh_db(
    db_path: &path::Path,
    options: FreshDbOptions,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), SqliteDbError> {
    create_db_file(db_path)?;

    let timestamp = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|dur| timestamp_to_u32(dur.as_secs()))
        .expect("System clock went backward the epoch?");

    // Fill the initial addresses. On a fresh database, the deposit_derivation_index is
    // necessarily 0.
    let mut query = String::with_capacity(100 * LOOK_AHEAD_LIMIT as usize);
    for index in 0..LOOK_AHEAD_LIMIT {
        let receive_address = options
            .main_descriptor
            .receive_descriptor()
            .derive(index.into(), secp)
            .address(options.bitcoind_network);
        let change_address = options
            .main_descriptor
            .change_descriptor()
            .derive(index.into(), secp)
            .address(options.bitcoind_network);
        query += &format!(
            "INSERT INTO addresses (receive_address, change_address, derivation_index) VALUES (\"{}\", \"{}\", {});\n",
            receive_address, change_address, index
        );
    }

    let mut conn = rusqlite::Connection::open(db_path)?;
    db_exec(&mut conn, |tx| {
        tx.execute_batch(SCHEMA)?;
        tx.execute(
            "INSERT INTO version (version) VALUES (?1)",
            rusqlite::params![DB_VERSION],
        )?;
        tx.execute(
            "INSERT INTO tip (network, blockheight, blockhash) VALUES (?1, NULL, NULL)",
            rusqlite::params![options.bitcoind_network.to_string()],
        )?;
        tx.execute(
            "INSERT INTO wallets (timestamp, main_descriptor, deposit_derivation_index, change_derivation_index) \
                     VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![timestamp, options.main_descriptor.to_string(), 0, 0],
        )?;
        tx.execute_batch(&query)?;

        Ok(())
    })?;

    Ok(())
}

/// Insert the deposit and change addresses for this index in the address->index mapping table
pub fn populate_address_mapping(
    db_tx: &rusqlite::Transaction,
    db_wallet: &DbWallet,
    next_index: u32,
    network: bitcoin::Network,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> rusqlite::Result<()> {
    // Update the address to derivation index mapping.
    let next_la_index = next_index + LOOK_AHEAD_LIMIT - 1;
    let next_receive_address = db_wallet
        .main_descriptor
        .receive_descriptor()
        .derive(next_la_index.into(), secp)
        .address(network);
    let next_change_address = db_wallet
        .main_descriptor
        .change_descriptor()
        .derive(next_la_index.into(), secp)
        .address(network);
    db_tx.execute(
            "INSERT INTO addresses (receive_address, change_address, derivation_index) VALUES (?1, ?2, ?3)",
            rusqlite::params![next_receive_address.to_string(), next_change_address.to_string(), next_la_index],
        )?;

    Ok(())
}
