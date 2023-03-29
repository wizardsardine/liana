use crate::database::sqlite::{schema::SCHEMA, FreshDbOptions, SqliteDbError, DB_VERSION};

use std::{convert::TryInto, fs, path, time};

use miniscript::bitcoin::secp256k1;

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

pub fn db_version(conn: &mut rusqlite::Connection) -> Result<i64, SqliteDbError> {
    Ok(db_query(
        conn,
        "SELECT version FROM version",
        rusqlite::params![],
        |row| {
            let version: i64 = row.get(0)?;
            Ok(version)
        },
    )?
    .pop()
    .expect("There is always a row in the version table"))
}

// In Liana 0.4 we upgraded the schema to hold a timestamp for transaction drafts. Existing
// transaction drafts are not set any timestamp on purpose.
fn migrate_v0_to_v1(conn: &mut rusqlite::Connection) -> Result<(), SqliteDbError> {
    db_exec(conn, |tx| {
        tx.execute(
            "ALTER TABLE spend_transactions ADD COLUMN updated_at",
            rusqlite::params![],
        )?;
        tx.execute(
            "UPDATE version SET version = 1",
            rusqlite::params![],
        )?;
        Ok(())
    })?;

    Ok(())
}

/// Check the database version and if necessary apply the migrations to upgrade it to the current
/// one.
pub fn maybe_apply_migration(db_path: &path::Path) -> Result<(), SqliteDbError> {
    let mut conn = rusqlite::Connection::open(db_path)?;

    // Iteratively apply the database migrations necessary.
    loop {
        let version = db_version(&mut conn)?;
        match version {
            DB_VERSION => {
                log::info!("Database is up to date.");
                return Ok(());
            }
            0 => {
                log::warn!("Upgrading database from version 0 to version 1.");
                migrate_v0_to_v1(&mut conn)?;
                log::warn!("Migration from database version 0 to version 1 successful.");
            }
            _ => return Err(SqliteDbError::UnsupportedVersion(version)),
        }
    }
}
