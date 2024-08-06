mod bitcoin;
pub mod commands;
pub mod config;
#[cfg(all(unix, feature = "daemon"))]
mod daemonize;
mod database;
pub mod descriptors;
#[cfg(feature = "daemon")]
mod jsonrpc;
pub mod random;
pub mod signer;
pub mod spend;
#[cfg(test)]
mod testutils;

pub use bip39;
pub use miniscript;

pub use crate::bitcoin::d::{BitcoinD, BitcoindError, WalletError};
#[cfg(feature = "daemon")]
use crate::jsonrpc::server::{rpcserver_loop, rpcserver_setup};
use crate::{
    bitcoin::{poller, BitcoinInterface},
    config::Config,
    database::{
        sqlite::{FreshDbOptions, SqliteDb, SqliteDbError, MAX_DB_VERSION_NO_TX_DB},
        DatabaseInterface,
    },
};

use std::{
    collections, error, fmt, fs, io, path,
    sync::{self, mpsc},
    thread,
};

use miniscript::bitcoin::secp256k1;

#[cfg(not(test))]
use std::panic;
// A panic in any thread should stop the main thread, and print the panic.
#[cfg(not(test))]
fn setup_panic_hook() {
    panic::set_hook(Box::new(move |panic_info| {
        let file = panic_info
            .location()
            .map(|l| l.file())
            .unwrap_or_else(|| "'unknown'");
        let line = panic_info
            .location()
            .map(|l| l.line().to_string())
            .unwrap_or_else(|| "'unknown'".to_string());

        let bt = backtrace::Backtrace::new();
        let info = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned());
        log::error!(
            "panic occurred at line {} of file {}: {:?}\n{:?}",
            line,
            file,
            info,
            bt
        );
    }));
}

#[derive(Debug, Clone)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}-dev", self.major, self.minor, self.patch)
    }
}

pub const VERSION: Version = Version {
    major: 6,
    minor: 0,
    patch: 0,
};

#[derive(Debug)]
pub enum StartupError {
    Io(io::Error),
    DefaultDataDirNotFound,
    DatadirCreation(path::PathBuf, io::Error),
    MissingBitcoindConfig,
    MissingBitcoinBackendConfig,
    DbMigrateBitcoinTxs(&'static str),
    Database(SqliteDbError),
    Bitcoind(BitcoindError),
    #[cfg(unix)]
    Daemonization(&'static str),
    #[cfg(windows)]
    NoWatchonlyInDatadir,
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{}", e),
            Self::DefaultDataDirNotFound => write!(
                f,
                "Not data directory was specified and a default path could not be determined for this platform."
            ),
            Self::DatadirCreation(dir_path, e) => write!(
                f,
                "Could not create data directory at '{}': '{}'", dir_path.display(), e
            ),
            Self::MissingBitcoindConfig => write!(
                f,
                "Our Bitcoin interface is bitcoind but we have no 'bitcoind_config' entry in the configuration."
            ),
            Self::MissingBitcoinBackendConfig => write!(
                f,
                "No Bitcoin backend entry in the configuration."
            ),
            Self::DbMigrateBitcoinTxs(msg) => write!(
                f,
                "Error when migrating Bitcoin transaction from Bitcoin backend to database: {}.", msg
            ),
            Self::Database(e) => write!(f, "Error initializing database: '{}'.", e),
            Self::Bitcoind(e) => write!(f, "Error setting up bitcoind interface: '{}'.", e),
            #[cfg(unix)]
            Self::Daemonization(e) => write!(f, "Error when daemonizing: '{}'.", e),
            #[cfg(windows)]
            Self::NoWatchonlyInDatadir => {
                write!(
                    f,
                    "A data directory exists with no watchonly wallet. Really old versions of Liana used to not \
                     store the bitcoind watchonly wallet under their own datadir on Windows. A migration will be \
                     necessary to be able to use such an old datadir with recent versions of Liana. The migration \
                     is automatically performed by Liana version 4 and older. If you want to salvage this datadir \
                     first run Liana v4 before running more recent Liana versions."
                )
            }
        }
    }
}

impl error::Error for StartupError {}

impl From<io::Error> for StartupError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<SqliteDbError> for StartupError {
    fn from(e: SqliteDbError) -> Self {
        Self::Database(e)
    }
}

impl From<BitcoindError> for StartupError {
    fn from(e: BitcoindError) -> Self {
        Self::Bitcoind(e)
    }
}

fn create_datadir(datadir_path: &path::Path) -> Result<(), StartupError> {
    #[cfg(unix)]
    return {
        use fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = DirBuilder::new();
        builder
            .mode(0o700)
            .recursive(true)
            .create(datadir_path)
            .map_err(|e| StartupError::DatadirCreation(datadir_path.to_path_buf(), e))
    };

    // TODO: permissions on Windows..
    #[cfg(not(unix))]
    return {
        fs::create_dir_all(datadir_path)
            .map_err(|e| StartupError::DatadirCreation(datadir_path.to_path_buf(), e))
    };
}

// Connect to the SQLite database. Create it if starting fresh, and do some sanity checks.
// If all went well, returns the interface to the SQLite database.
fn setup_sqlite(
    config: &Config,
    data_dir: &path::Path,
    fresh_data_dir: bool,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    bitcoind: &Option<BitcoinD>,
) -> Result<SqliteDb, StartupError> {
    let db_path: path::PathBuf = [data_dir, path::Path::new("lianad.sqlite3")]
        .iter()
        .collect();
    let options = if fresh_data_dir {
        Some(FreshDbOptions::new(
            config.bitcoin_config.network,
            config.main_descriptor.clone(),
        ))
    } else {
        None
    };

    // If opening an existing wallet whose database does not yet store the wallet transactions,
    // query them from the Bitcoin backend before proceeding to the migration.
    let sqlite = SqliteDb::new(db_path, options, secp)?;
    if !fresh_data_dir {
        let mut conn = sqlite.connection()?;
        let wallet_txs = if conn.db_version() <= MAX_DB_VERSION_NO_TX_DB {
            let bit = bitcoind.as_ref().ok_or(StartupError::DbMigrateBitcoinTxs(
                "a connection to a Bitcoin backend is required",
            ))?;
            let coins = conn.db_coins(&[]);
            let coins_txids = coins
                .iter()
                .map(|c| c.outpoint.txid)
                .chain(coins.iter().filter_map(|c| c.spend_txid))
                .collect::<collections::HashSet<_>>();
            coins_txids
                .into_iter()
                .map(|txid| bit.get_transaction(&txid).map(|res| res.tx))
                .collect::<Option<Vec<_>>>()
                .ok_or(StartupError::DbMigrateBitcoinTxs(
                    "missing transaction in Bitcoin backend",
                ))?
        } else {
            Vec::new()
        };
        sqlite.maybe_apply_migrations(&wallet_txs)?;
    }

    sqlite.sanity_check(config.bitcoin_config.network, &config.main_descriptor)?;
    log::info!("Database initialized and checked.");

    Ok(sqlite)
}

// Connect to bitcoind. Setup the watchonly wallet, and do some sanity checks.
// If all went well, returns the interface to bitcoind.
fn setup_bitcoind(
    config: &Config,
    data_dir: &path::Path,
    fresh_data_dir: bool,
) -> Result<BitcoinD, StartupError> {
    let wo_path: path::PathBuf = [data_dir, path::Path::new("lianad_watchonly_wallet")]
        .iter()
        .collect();
    let wo_path_str = wo_path.to_str().expect("Must be valid unicode").to_string();
    // NOTE: On Windows, paths are canonicalized with a "\\?\" prefix to tell Windows to interpret
    // the string "as is" and to ignore the maximum size of a path. HOWEVER this is not properly
    // handled by most implementations of the C++ STL's std::filesystem. Therefore bitcoind would
    // fail to find the wallet if we didn't strip this prefix. It's not ideal, but a lesser evil
    // than other workarounds i could think about.
    // See https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file#win32-file-namespaces
    // about the prefix.
    // See https://stackoverflow.com/questions/71590689/how-to-properly-handle-windows-paths-with-the-long-path-prefix-with-stdfilesys
    // for a discussion of how one C++ STL implementation handles this.
    #[cfg(target_os = "windows")]
    let wo_path_str = wo_path_str.replace("\\\\?\\", "").replace("\\\\?", "");

    let config::BitcoinBackend::Bitcoind(bitcoind_config) = config
        .bitcoin_backend
        .as_ref()
        .ok_or(StartupError::MissingBitcoindConfig)?;
    let bitcoind = BitcoinD::new(bitcoind_config, wo_path_str)?;
    bitcoind.node_sanity_checks(
        config.bitcoin_config.network,
        config.main_descriptor.is_taproot(),
    )?;
    if fresh_data_dir {
        log::info!("Creating a new watchonly wallet on bitcoind.");
        bitcoind.create_watchonly_wallet(&config.main_descriptor)?;
        log::info!("Watchonly wallet created.");
    } else {
        #[cfg(windows)]
        if !cfg!(test) && !wo_path.exists() {
            return Err(StartupError::NoWatchonlyInDatadir);
        }
    }
    log::info!("Loading our watchonly wallet on bitcoind.");
    bitcoind.maybe_load_watchonly_wallet()?;
    bitcoind.wallet_sanity_checks(&config.main_descriptor)?;
    log::info!("Watchonly wallet loaded on bitcoind and sanity checked.");

    Ok(bitcoind)
}

#[derive(Clone)]
pub struct DaemonControl {
    config: Config,
    bitcoin: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    poller_sender: mpsc::SyncSender<poller::PollerMessage>,
    // FIXME: Should we require Sync on DatabaseInterface rather than using a Mutex?
    db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
}

impl DaemonControl {
    pub(crate) fn new(
        config: Config,
        bitcoin: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        poller_sender: mpsc::SyncSender<poller::PollerMessage>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) -> DaemonControl {
        DaemonControl {
            config,
            bitcoin,
            poller_sender,
            db,
            secp,
        }
    }

    // Useful for unit test to directly mess up with the DB
    #[cfg(test)]
    pub fn db(&self) -> sync::Arc<sync::Mutex<dyn DatabaseInterface>> {
        self.db.clone()
    }
}

/// The handle to a Liana daemon. It might either be the handle for a daemon which exposes a
/// JSONRPC server or one which exposes its API through a `DaemonControl`.
pub enum DaemonHandle {
    Controller {
        poller_sender: mpsc::SyncSender<poller::PollerMessage>,
        poller_handle: thread::JoinHandle<()>,
        control: DaemonControl,
    },
    #[cfg(feature = "daemon")]
    Server {
        poller_sender: mpsc::SyncSender<poller::PollerMessage>,
        poller_handle: thread::JoinHandle<()>,
        rpcserver_shutdown: sync::Arc<sync::atomic::AtomicBool>,
        rpcserver_handle: thread::JoinHandle<Result<(), io::Error>>,
    },
}

impl DaemonHandle {
    /// This starts the Liana daemon. A user of this interface should regularly poll the `is_alive`
    /// method to check for internal errors. To shut down the daemon use the `stop` method.
    ///
    /// The `with_rpc_server` controls whether we should start a JSONRPC server to receive queries
    /// or instead return a `DaemonControl` object for a caller to access the daemon's API.
    ///
    /// You may specify a custom Bitcoin interface through the `bitcoin` parameter. If `None`, the
    /// default Bitcoin interface (`bitcoind` JSONRPC) will be used.
    /// You may specify a custom Database interface through the `db` parameter. If `None`, the
    /// default Database interface (SQLite) will be used.
    pub fn start(
        config: Config,
        bitcoin: Option<impl BitcoinInterface + 'static>,
        db: Option<impl DatabaseInterface + 'static>,
        #[cfg(feature = "daemon")] with_rpc_server: bool,
    ) -> Result<Self, StartupError> {
        #[cfg(not(test))]
        setup_panic_hook();

        let secp = secp256k1::Secp256k1::verification_only();

        // First, check the data directory
        let mut data_dir = config
            .data_dir()
            .ok_or(StartupError::DefaultDataDirNotFound)?;
        data_dir.push(config.bitcoin_config.network.to_string());
        let fresh_data_dir = !data_dir.as_path().exists();
        if fresh_data_dir {
            create_datadir(&data_dir)?;
            log::info!("Created a new data directory at '{}'", data_dir.display());
        }

        // Set up the connection to bitcoind (if using it) first as we may need it for the database
        // migration when setting up SQLite below.
        let bitcoind = if bitcoin.is_none() {
            if let Some(config::BitcoinBackend::Bitcoind(_)) = &config.bitcoin_backend {
                Some(setup_bitcoind(&config, &data_dir, fresh_data_dir)?)
            } else {
                None
            }
        } else {
            None
        };

        // Then set up the database backend.
        let db = match db {
            Some(db) => sync::Arc::from(sync::Mutex::from(db)),
            None => sync::Arc::from(sync::Mutex::from(setup_sqlite(
                &config,
                &data_dir,
                fresh_data_dir,
                &secp,
                &bitcoind,
            )?)) as sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        };

        // Finally set up the Bitcoin backend.
        let bit = match (bitcoin, &config.bitcoin_backend) {
            (Some(bit), _) => sync::Arc::from(sync::Mutex::from(bit)),
            (None, Some(config::BitcoinBackend::Bitcoind(..))) => sync::Arc::from(
                sync::Mutex::from(bitcoind.expect("bitcoind must have been set already")),
            )
                as sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
            (None, None) => Err(StartupError::MissingBitcoinBackendConfig)?,
        };

        // If we are on a UNIX system and they told us to daemonize, do it now.
        // NOTE: it's safe to daemonize now, as we don't carry any open DB connection
        // https://www.sqlite.org/howtocorrupt.html#_carrying_an_open_database_connection_across_a_fork_
        #[cfg(all(unix, feature = "daemon"))]
        if config.daemon {
            log::info!("Daemonizing");
            let log_file = data_dir.as_path().join("log");
            let pid_file = data_dir.as_path().join("lianad.pid");
            unsafe {
                daemonize::daemonize(&data_dir, &log_file, &pid_file)
                    .map_err(StartupError::Daemonization)?;
            }
        }

        // Start the poller thread. Keep the thread handle to be able to check if it crashed. Store
        // an atomic to be able to stop it.
        let mut bitcoin_poller =
            poller::Poller::new(bit.clone(), db.clone(), config.main_descriptor.clone());
        let (poller_sender, poller_receiver) = mpsc::sync_channel(0);
        let poller_handle = thread::Builder::new()
            .name("Bitcoin Network poller".to_string())
            .spawn({
                let poll_interval = config.bitcoin_config.poll_interval_secs;
                move || {
                    log::info!("Bitcoin poller started.");
                    bitcoin_poller.poll_forever(poll_interval, poller_receiver);
                    log::info!("Bitcoin poller stopped.");
                }
            })
            .expect("Spawning the poller thread must never fail.");

        // Create the API the external world will use to talk to us, either directly through the Rust
        // structure or through the JSONRPC server we may setup below.
        let control = DaemonControl::new(config, bit, poller_sender.clone(), db, secp);

        #[cfg(feature = "daemon")]
        if with_rpc_server {
            let rpcserver_shutdown = sync::Arc::from(sync::atomic::AtomicBool::from(false));
            let rpcserver_handle = thread::Builder::new()
                .name("Bitcoin Network poller".to_string())
                .spawn({
                    let shutdown = rpcserver_shutdown.clone();
                    move || {
                        let mut rpc_socket = data_dir;
                        rpc_socket.push("lianad_rpc");
                        let listener = rpcserver_setup(&rpc_socket)?;
                        log::info!("JSONRPC server started.");

                        rpcserver_loop(listener, control, shutdown)?;
                        log::info!("JSONRPC server stopped.");
                        Ok(())
                    }
                })
                .expect("Spawning the RPC server thread should never fail.");

            return Ok(DaemonHandle::Server {
                poller_sender,
                poller_handle,
                rpcserver_shutdown,
                rpcserver_handle,
            });
        }

        Ok(DaemonHandle::Controller {
            poller_sender,
            poller_handle,
            control,
        })
    }

    /// Start the Liana daemon with the default Bitcoin and database interfaces (`bitcoind` RPC
    /// and SQLite).
    pub fn start_default(
        config: Config,
        #[cfg(feature = "daemon")] with_rpc_server: bool,
    ) -> Result<DaemonHandle, StartupError> {
        Self::start(
            config,
            Option::<BitcoinD>::None,
            Option::<SqliteDb>::None,
            #[cfg(feature = "daemon")]
            with_rpc_server,
        )
    }

    /// Check whether the daemon is still up and running. This needs to be regularly polled to
    /// check for internal errors. If this returns `false`, collect the error using the `stop`
    /// method.
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Controller {
                ref poller_handle, ..
            } => !poller_handle.is_finished(),
            #[cfg(feature = "daemon")]
            Self::Server {
                ref poller_handle,
                ref rpcserver_handle,
                ..
            } => !poller_handle.is_finished() && !rpcserver_handle.is_finished(),
        }
    }

    /// Stop the Liana daemon. This returns any error which may have occurred.
    pub fn stop(self) -> Result<(), Box<dyn error::Error>> {
        match self {
            Self::Controller {
                poller_sender,
                poller_handle,
                ..
            } => {
                poller_sender
                    .send(poller::PollerMessage::Shutdown)
                    .expect("The other end should never have hung up before this.");
                poller_handle.join().expect("Poller thread must not panic");
                Ok(())
            }
            #[cfg(feature = "daemon")]
            Self::Server {
                poller_sender,
                poller_handle,
                rpcserver_shutdown,
                rpcserver_handle,
            } => {
                poller_sender
                    .send(poller::PollerMessage::Shutdown)
                    .expect("The other end should never have hung up before this.");
                rpcserver_shutdown.store(true, sync::atomic::Ordering::Relaxed);
                rpcserver_handle
                    .join()
                    .expect("Poller thread must not panic")?;
                poller_handle.join().expect("Poller thread must not panic");
                Ok(())
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        config::{BitcoinConfig, BitcoindConfig, BitcoindRpcAuth},
        descriptors::LianaDescriptor,
        testutils::*,
    };

    use miniscript::bitcoin;
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        net, path,
        str::FromStr,
        thread, time,
    };

    // Read all bytes from the socket until the end of a JSON object, good enough approximation.
    fn read_til_json_end(stream: &mut net::TcpStream) {
        stream
            .set_read_timeout(Some(time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            if line.starts_with("Authorization") {
                let mut buf = vec![0; 256];
                reader.read_until(b'}', &mut buf).unwrap();
                return;
            }
        }
    }

    // Respond to the two "echo" sent at startup to sanity check the connection
    fn complete_sanity_check(server: &net::TcpListener) {
        let echo_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[]}\n".as_bytes();

        // Read the first echo, respond to it
        {
            let (mut stream, _) = server.accept().unwrap();
            read_til_json_end(&mut stream);
            stream.write_all(echo_resp).unwrap();
            stream.flush().unwrap();
        }

        // Read the second echo, respond to it
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(echo_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a pruned getblockchaininfo telling them we are at version 24.0
    fn complete_version_check(server: &net::TcpListener) {
        let net_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"version\":240000}}\n"
                .as_bytes();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a pruned getblockchaininfo telling them we are on mainnet
    fn complete_network_check(server: &net::TcpListener) {
        let net_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"chain\":\"main\"}}\n"
                .as_bytes();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them responses for the calls involved when creating a fresh wallet
    fn complete_wallet_creation(server: &net::TcpListener) {
        {
            let net_resp =
                ["HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[]}\n".as_bytes()]
                    .concat();
            let (mut stream, _) = server.accept().unwrap();
            read_til_json_end(&mut stream);
            stream.write_all(&net_resp).unwrap();
            stream.flush().unwrap();
        }

        {
            let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"name\":\"dummy\"}}\n"
                .as_bytes(),
            ]
            .concat();
            let (mut stream, _) = server.accept().unwrap();
            read_til_json_end(&mut stream);
            stream.write_all(&net_resp).unwrap();
            stream.flush().unwrap();
        }

        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[{\"success\":true}]}\n"
                .as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a dummy result to loadwallet.
    fn complete_wallet_loading(server: &net::TcpListener) {
        {
            let listwallets_resp =
                "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[]}\n".as_bytes();
            let (mut stream, _) = server.accept().unwrap();
            read_til_json_end(&mut stream);
            stream.write_all(listwallets_resp).unwrap();
            stream.flush().unwrap();
        }

        let loadwallet_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"name\":\"dummy\"}}\n"
                .as_bytes();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(loadwallet_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a response to 'listwallets' with the watchonly wallet path
    fn complete_wallet_check(server: &net::TcpListener, watchonly_wallet_path: &str) {
        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[\"".as_bytes(),
            watchonly_wallet_path.as_bytes(),
            "\"]}\n".as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a response to 'listdescriptors' with the receive and change descriptors
    fn complete_desc_check(server: &net::TcpListener, receive_desc: &str, change_desc: &str) {
        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"descriptors\":[{\"desc\":\"".as_bytes(),
            receive_desc.as_bytes(),
            "\",\"timestamp\":0},".as_bytes(),
            "{\"desc\":\"".as_bytes(),
            change_desc.as_bytes(),
            "\",\"timestamp\":1}]}}\n".as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a response to 'getblockhash' with the genesis block hash
    fn complete_tip_init(server: &net::TcpListener) {
        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f\"}\n".as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    // TODO: we could move the dummy bitcoind thread stuff to the bitcoind module to test the
    // bitcoind interface, and use the DummyLiana from testutils to sanity check the startup.
    // Note that startup as checked by this unit test is also tested in the functional test
    // framework.
    #[test]
    fn daemon_startup() {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let data_dir: path::PathBuf = [tmp_dir.as_path(), path::Path::new("datadir")]
            .iter()
            .collect();
        let wo_path: path::PathBuf = [
            data_dir.as_path(),
            path::Path::new("bitcoin"),
            path::Path::new("lianad_watchonly_wallet"),
        ]
        .iter()
        .collect();
        let wo_path = wo_path.to_str().unwrap().to_string();

        // Configure a dummy bitcoind
        let network = bitcoin::Network::Bitcoin;
        let cookie: path::PathBuf = [
            tmp_dir.as_path(),
            path::Path::new(&format!(
                "dummy_bitcoind_{:?}.cookie",
                thread::current().id()
            )),
        ]
        .iter()
        .collect();
        fs::write(&cookie, [0; 32]).unwrap(); // Will overwrite should it exist already
        let addr: net::SocketAddr =
            net::SocketAddrV4::new(net::Ipv4Addr::new(127, 0, 0, 1), 0).into();
        let server = net::TcpListener::bind(addr).unwrap();
        let addr = server.local_addr().unwrap();
        let bitcoin_config = BitcoinConfig {
            network,
            poll_interval_secs: time::Duration::from_secs(2),
        };
        let bitcoind_config = BitcoindConfig {
            addr,
            rpc_auth: BitcoindRpcAuth::CookieFile(cookie),
        };

        // Create a dummy config with this bitcoind
        let desc_str = "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn";
        let desc = LianaDescriptor::from_str(desc_str).unwrap();
        let receive_desc = desc.receive_descriptor().clone();
        let change_desc = desc.change_descriptor().clone();
        let config = Config {
            bitcoin_config,
            bitcoin_backend: Some(config::BitcoinBackend::Bitcoind(bitcoind_config)),
            data_dir: Some(data_dir),
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Debug,
            main_descriptor: desc,
        };

        // Start the daemon in a new thread so the current one acts as the bitcoind server.
        let t = thread::spawn({
            let config = config.clone();
            move || {
                let handle = DaemonHandle::start_default(
                    config,
                    #[cfg(feature = "daemon")]
                    false,
                )
                .unwrap();
                handle.stop().unwrap();
            }
        });
        complete_sanity_check(&server);
        complete_version_check(&server);
        complete_network_check(&server);
        complete_wallet_creation(&server);
        complete_wallet_loading(&server);
        complete_wallet_check(&server, &wo_path);
        complete_desc_check(&server, &receive_desc.to_string(), &change_desc.to_string());
        complete_tip_init(&server);
        // We don't have to complete the sync check as the poller checks whether it needs to stop
        // before checking the bitcoind sync status.
        t.join().unwrap();

        // The datadir is created now, so if we restart it it won't create the wo wallet.
        let t = thread::spawn({
            let config = config.clone();
            move || {
                let handle = DaemonHandle::start_default(
                    config,
                    #[cfg(feature = "daemon")]
                    false,
                )
                .unwrap();
                handle.stop().unwrap();
            }
        });
        complete_sanity_check(&server);
        complete_version_check(&server);
        complete_network_check(&server);
        complete_wallet_loading(&server);
        complete_wallet_check(&server, &wo_path);
        complete_desc_check(&server, &receive_desc.to_string(), &change_desc.to_string());
        // We don't have to complete the sync check as the poller checks whether it needs to stop
        // before checking the bitcoind sync status.
        t.join().unwrap();

        fs::remove_dir_all(&tmp_dir).unwrap();
    }
}
