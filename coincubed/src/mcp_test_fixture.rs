//! Disposable SQLite wallet for desktop MCP tests; never built into production.
use crate::{
    config::{BitcoinConfig, Config},
    database::{
        sqlite::{FreshDbOptions, SqliteDb},
        Coin, DatabaseConnection,
    },
    datadir::DataDirectory,
    testutils::DummyBitcoind,
    DaemonHandle,
};
use miniscript::bitcoin::{self, bip32::ChildNumber, hashes::Hash, secp256k1::Secp256k1};
use std::time::Duration;
/// Create a disposable SQLite Vault with one synthetic confirmed 55,000 sat output.
/// Chain polling uses DummyBitcoind; no external node, funds or credentials are used.
/// The caller must stop and drop the returned daemon before removing its directory.
pub fn wallet(
    root: &std::path::Path,
    descriptor: coincube_core::descriptors::CoincubeDescriptor,
) -> (Config, DaemonHandle) {
    wallet_on_chain(root, descriptor, bitcoin::Network::Bitcoin.into())
}
/// Same disposable wallet engine, explicitly bound to Bitcoin Regtest.
pub fn regtest_wallet(
    root: &std::path::Path,
    descriptor: coincube_core::descriptors::CoincubeDescriptor,
) -> (Config, DaemonHandle) {
    wallet_on_chain(root, descriptor, bitcoin::Network::Regtest.into())
}
/// Simulate an input disappearing from the disposable wallet's coin set.
/// Test-only: this changes only the fixture's SQLite database, never a node.
pub fn remove_coin(root: &std::path::Path, outpoint: bitcoin::OutPoint) {
    let database = SqliteDb::new(
        root.join("wallet.sqlite3"),
        None,
        &Secp256k1::verification_only(),
    )
    .unwrap();
    database.connection().unwrap().remove_coins(&[outpoint]);
}
/// Apply a same-height tip replacement without advancing the poll timestamp.
pub fn replace_tip_hash_same_height(root: &std::path::Path) {
    let database = SqliteDb::new(
        root.join("wallet.sqlite3"),
        None,
        &Secp256k1::verification_only(),
    )
    .unwrap();
    let mut connection = database.connection().unwrap();
    let tip = connection.chain_tip().unwrap();
    connection.update_tip(&crate::bitcoin::BlockChainTip {
        hash: bitcoin::BlockHash::all_zeros(),
        height: tip.height,
    });
}
/// Public-only synthetic chain fixture. This module is compiled only with
/// test-utils; supplied DummyBitcoind never contacts a node or moves funds.
/// Production fork admission is tested separately and remains mandatory.
pub fn wallet_on_chain(
    root: &std::path::Path,
    descriptor: coincube_core::descriptors::CoincubeDescriptor,
    chain: coincube_core::chain::ChainId,
) -> (Config, DaemonHandle) {
    let secp = Secp256k1::verification_only();
    let database = SqliteDb::new(
        root.join("wallet.sqlite3"),
        Some(FreshDbOptions::new(chain, descriptor.clone())),
        &secp,
    )
    .unwrap();
    let mut connection = database.connection().unwrap();
    let script = descriptor
        .receive_descriptor()
        .derive(0.into(), &secp)
        .script_pubkey();
    let transaction = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint {
                txid: bitcoin::Txid::all_zeros(),
                vout: 1,
            },
            ..Default::default()
        }],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(55_000),
                script_pubkey: script,
            },
            // Unrelated sender change must never become outgoing wallet funds.
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(945_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let outpoint = bitcoin::OutPoint {
        txid: transaction.compute_txid(),
        vout: 0,
    };
    connection.new_txs(&[transaction]);
    connection.new_unspent_coins(&[Coin {
        outpoint,
        is_immature: false,
        block_info: None,
        amount: bitcoin::Amount::from_sat(55_000),
        derivation_index: ChildNumber::from(0),
        is_change: false,
        spend_txid: None,
        spend_block: None,
        is_from_self: false,
    }]);
    connection.confirm_coins(&[(outpoint, 1, 1)]);
    connection.update_tip(&DummyBitcoind::new().tip);
    {
        // This fixture's synthetic confirmed coins represent a completed poll.
        // Populate its corresponding readiness metadata deterministically;
        // production synchronization checks are not bypassed.
        connection.set_wallet_last_poll_timestamp(1).unwrap();
    }
    drop(connection);
    let config = Config::new(
        BitcoinConfig::new(chain, Duration::from_secs(3600)),
        None,
        log::LevelFilter::Warn,
        descriptor.clone(),
        DataDirectory::new(root.join("daemon")),
    );
    let handle = if chain.is_blake2b() {
        synthetic_fork_handle(config.clone(), database)
    } else {
        DaemonHandle::start(
            config.clone(),
            Some(DummyBitcoind::new()),
            Some(database),
            false,
        )
        .unwrap()
    };
    {
        // MCP history and transaction tests start from a polled wallet. The poller runs
        // on another thread, so waiting for its first scheduled tick in each
        // caller can time out on a busy CI runner before it publishes sync.
        let (done, received) = std::sync::mpsc::sync_channel(0);
        let DaemonHandle::Controller { poller_sender, .. } = &handle else {
            unreachable!("the test fixture starts an embedded daemon");
        };
        poller_sender
            .send(crate::bitcoin::poller::PollerMessage::PollNow(done))
            .expect("request synthetic wallet poll");
        received
            .recv_timeout(Duration::from_secs(30))
            .expect("synthetic wallet poll completed");
    }
    (config, handle)
}

/// Only the test-utils feature compiles this module. Construct a synthetic
/// fork controller with the real command/SQLite/poller implementation, without
/// claiming to exercise network or production backend admission. Production
/// start/start_local_fork checks are unchanged.
fn synthetic_fork_handle(config: Config, database: SqliteDb) -> DaemonHandle {
    use std::sync::{atomic::AtomicBool, mpsc, Arc, Mutex};
    let bitcoin: Arc<Mutex<dyn crate::bitcoin::BitcoinInterface>> =
        Arc::new(Mutex::new(DummyBitcoind::new()));
    let db: Arc<Mutex<dyn crate::database::DatabaseInterface>> = Arc::new(Mutex::new(database));
    let sync_progress = Arc::new(crate::bitcoin::SyncProgressCache::default());
    let reorg_alert = Arc::new(crate::bitcoin::ReorgAlertCache::default());
    let history_sync = Arc::new(crate::bitcoin::HistorySyncCache::default());
    let tip_time = Arc::new(crate::bitcoin::TipTimeCache::default());
    let mut poller = crate::poller::Poller::new(
        bitcoin.clone(),
        db.clone(),
        config.main_descriptor.clone(),
        sync_progress.clone(),
        reorg_alert.clone(),
        history_sync.clone(),
        tip_time.clone(),
    )
    .unwrap();
    let (poller_sender, receiver) = mpsc::sync_channel(1);
    let poller_handle = std::thread::Builder::new()
        .name("MCP synthetic fork poller".into())
        .spawn(move || poller.poll_forever(Duration::from_secs(3600), receiver))
        .unwrap();
    let control = crate::DaemonControl::new(
        config,
        bitcoin,
        poller_sender.clone(),
        db,
        Secp256k1::verification_only(),
        sync_progress,
        reorg_alert,
        history_sync,
        tip_time,
    );
    DaemonHandle::Controller {
        poller_sender,
        poller_handle,
        control,
        scan_abort: Arc::new(AtomicBool::new(false)),
        datadir_lock: None,
    }
}
