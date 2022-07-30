mod bitcoin;
pub mod config;
#[cfg(unix)]
mod daemonize;
mod database;
pub mod descriptors;

pub use miniscript;

use crate::{
    bitcoin::{
        d::{BitcoinD, BitcoindError},
        poller,
    },
    config::{config_folder_path, Config},
    database::sqlite::{FreshDbOptions, SqliteDb, SqliteDbError},
};

use std::{error, fmt, fs, io, path, sync};

#[cfg(not(test))]
use std::{panic, process};
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

        process::exit(1);
    }));
}

#[derive(Debug)]
pub enum StartupError {
    Io(io::Error),
    DefaultDataDirNotFound,
    DatadirCreation(path::PathBuf, io::Error),
    Database(SqliteDbError),
    Bitcoind(BitcoindError),
    #[cfg(unix)]
    Daemonization(&'static str),
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
            Self::Database(e) => write!(f, "Error initializing database: '{}'.", e),
            Self::Bitcoind(e) => write!(f, "Error setting up bitcoind interface: '{}'.", e),
            #[cfg(unix)]
            Self::Daemonization(e) => write!(f, "Error when daemonizing: '{}'.", e),
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

pub struct DaemonHandle {}

impl DaemonHandle {
    /// This starts the Minisafe daemon. Call `shutdown` to shut it down.
    ///
    /// **Note**: we internally use threads, and set a panic hook. A downstream application must
    /// not overwrite this panic hook.
    pub fn start(config: Config) -> Result<Self, StartupError> {
        #[cfg(not(test))]
        setup_panic_hook();

        // First, check the data directory
        let mut data_dir = config
            .data_dir
            .unwrap_or(config_folder_path().ok_or(StartupError::DefaultDataDirNotFound)?);
        data_dir.push(config.bitcoind_config.network.to_string());
        let fresh_data_dir = !data_dir.as_path().exists();
        if fresh_data_dir {
            create_datadir(&data_dir)?;
            log::info!("Created a new data directory at '{}'", data_dir.display());
        }

        // Then set up the database
        let db_path: path::PathBuf = [data_dir.as_path(), path::Path::new("minisafed.sqlite3")]
            .iter()
            .collect();
        let options = if fresh_data_dir {
            Some(FreshDbOptions {
                bitcoind_network: config.bitcoind_config.network,
                main_descriptor: config.main_descriptor.clone(),
            })
        } else {
            None
        };
        let db = SqliteDb::new(db_path, options)?;
        db.sanity_check(config.bitcoind_config.network, &config.main_descriptor)?;
        log::info!("Database initialized and checked.");

        // Now set up the bitcoind interface
        let wo_path: path::PathBuf = [
            data_dir.as_path(),
            path::Path::new("minisafed_watchonly_wallet"),
        ]
        .iter()
        .collect();
        let bitcoind = BitcoinD::new(
            &config.bitcoind_config,
            wo_path.to_str().expect("Must be valid unicode").to_string(),
        )?;
        if fresh_data_dir {
            bitcoind.create_watchonly_wallet(&config.main_descriptor)?;
            log::info!("Created a new watchonly wallet on bitcoind.");
        }
        bitcoind.try_load_watchonly_wallet();
        bitcoind.sanity_check(&config.main_descriptor, config.bitcoind_config.network)?;
        log::info!("Connection to bitcoind established and checked.");

        // If we are on a UNIX system and they told us to daemonize, do it now.
        // NOTE: it's safe to daemonize now, as we don't carry any open DB connection
        // https://www.sqlite.org/howtocorrupt.html#_carrying_an_open_database_connection_across_a_fork_
        #[cfg(unix)]
        if config.daemon {
            log::info!("Daemonizing");
            let log_file = data_dir.as_path().join("log");
            let pid_file = data_dir.as_path().join("revaultd.pid");
            unsafe {
                daemonize::daemonize(&data_dir, &log_file, &pid_file)
                    .map_err(StartupError::Daemonization)?;
            }
        }

        // Spawn the bitcoind poller with a retry limit high enough that we'd fail after that.
        let bitcoind = sync::Arc::from(sync::RwLock::from(bitcoind.with_retry_limit(None)));
        let bit_poller = poller::Poller::start(
            bitcoind.clone(),
            db,
            config.bitcoind_config.poll_interval_secs,
        );
        bit_poller.stop();

        Ok(Self {})
    }

    // NOTE: this moves out the data as it should not be reused after shutdown
    /// Shut down the Minisafe daemon.
    pub fn shutdown(self) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::BitcoindConfig;

    use miniscript::{bitcoin, Descriptor, DescriptorPublicKey};
    use std::{
        env, fs,
        io::{BufRead, BufReader, Write},
        net, path, process,
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
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(echo_resp).unwrap();
        stream.flush().unwrap();

        // Read the second echo, respond to it
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(echo_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a pruned getblockchaininfo telling them we are at version 23.99
    fn complete_version_check(server: &net::TcpListener) {
        let net_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"version\":239900}}\n"
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
        let net_resp =
            ["HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[]}\n".as_bytes()]
                .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();

        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"name\":\"dummy\"}}\n"
                .as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();

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
        let net_resp =
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"name\":\"dummy\"}}\n"
                .as_bytes();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a response to 'listwallets' with the watchonly wallet path
    fn complete_wallet_check<'a>(server: &net::TcpListener, watchonly_wallet_path: &'a str) {
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

    // Send them a response to 'listdescriptors' with the main descriptor
    fn complete_desc_check<'a>(server: &net::TcpListener, desc: &'a str) {
        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"descriptors\":[{\"desc\":\"".as_bytes(),
            desc.as_bytes(),
            "\"}]}}\n".as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    // Send them a response to 'getblockchaininfo' saying we are far from being synced
    fn complete_sync_check<'a>(server: &net::TcpListener) {
        let net_resp = [
            "HTTP/1.1 200\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"verificationprogress\":0.1}}\n".as_bytes(),
        ]
        .concat();
        let (mut stream, _) = server.accept().unwrap();
        read_til_json_end(&mut stream);
        stream.write_all(&net_resp).unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn daemon_startup() {
        let tmp_dir = env::temp_dir().join(format!(
            "minisafed-unit-tests-{}-{:?}",
            process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&tmp_dir).unwrap();
        let data_dir: path::PathBuf = [tmp_dir.as_path(), path::Path::new("datadir")]
            .iter()
            .collect();
        let wo_path: path::PathBuf = [
            data_dir.as_path(),
            path::Path::new("bitcoin"),
            path::Path::new("minisafed_watchonly_wallet"),
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
        fs::write(&cookie, &[0; 32]).unwrap(); // Will overwrite should it exist already
        let addr: net::SocketAddr =
            net::SocketAddrV4::new(net::Ipv4Addr::new(127, 0, 0, 1), 0).into();
        let server = net::TcpListener::bind(&addr).unwrap();
        let addr = server.local_addr().unwrap();
        let bitcoind_config = BitcoindConfig {
            network,
            addr,
            cookie_path: cookie.clone(),
            poll_interval_secs: time::Duration::from_secs(2),
        };

        // Create a dummy config with this bitcoind
        let desc_str = "wsh(andor(pk(03b506a1dbe57b4bf48c95e0c7d417b87dd3b4349d290d2e7e9ba72c912652d80a),older(10000),pk(0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce)))#39x77spy";
        let desc = Descriptor::<DescriptorPublicKey>::from_str(desc_str).unwrap();
        let config = Config {
            bitcoind_config,
            data_dir: Some(data_dir.clone()),
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Debug,
            main_descriptor: desc,
        };

        // Start the daemon in a new thread so the current one acts as the bitcoind server.
        let daemon_thread = thread::spawn({
            let config = config.clone();
            move || {
                let handle = DaemonHandle::start(config).unwrap();
                handle.shutdown();
            }
        });
        complete_sanity_check(&server);
        complete_wallet_creation(&server);
        complete_wallet_loading(&server);
        complete_version_check(&server);
        complete_network_check(&server);
        complete_wallet_check(&server, &wo_path);
        complete_desc_check(&server, desc_str);
        complete_sync_check(&server);
        daemon_thread.join().unwrap();

        // The datadir is created now, so if we restart it it won't create the wo wallet.
        let daemon_thread = thread::spawn(move || {
            let handle = DaemonHandle::start(config).unwrap();
            handle.shutdown();
        });
        complete_sanity_check(&server);
        complete_wallet_loading(&server);
        complete_version_check(&server);
        complete_network_check(&server);
        complete_wallet_check(&server, &wo_path);
        complete_desc_check(&server, desc_str);
        complete_sync_check(&server);
        daemon_thread.join().unwrap();

        fs::remove_dir_all(&tmp_dir).unwrap();
    }
}
