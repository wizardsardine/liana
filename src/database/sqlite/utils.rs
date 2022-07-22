use crate::database::sqlite::{schema::SCHEMA, FreshDbOptions, SqliteDbError, DB_VERSION};

use std::{convert::TryInto, fs, path, time};

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
    // rustc says 'borrowed value does not live long enough'
    let x = conn
        .prepare(stmt_str)?
        .query_map(params, f)?
        .collect::<rusqlite::Result<Vec<T>>>();

    x
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

pub fn create_fresh_db(db_path: &path::Path, options: FreshDbOptions) -> Result<(), SqliteDbError> {
    create_db_file(db_path)?;

    let timestamp = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|dur| timestamp_to_u32(dur.as_secs()))
        .expect("System clock went backward the epoch?");

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
            "INSERT INTO wallets (timestamp, main_descriptor, deposit_derivation_index) \
                     VALUES (?1, ?2, ?3)",
            rusqlite::params![timestamp, options.main_descriptor.to_string(), 0,],
        )?;

        Ok(())
    })?;

    Ok(())
}
