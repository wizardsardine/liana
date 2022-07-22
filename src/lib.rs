pub mod config;
mod database;

use crate::{
    config::{config_folder_path, Config},
    database::sqlite::{FreshDbOptions, SqliteDb, SqliteDbError},
};

use std::{error, fmt, fs, io, path};

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
            Self::Database(e) => write!(f, "Error initializing database: '{}'.", e)
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

        Ok(Self {})
    }

    // NOTE: this moves out the data as it should not be reused after shutdown
    /// Shut down the Minisafe daemon.
    pub fn shutdown(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BitcoindConfig;

    use miniscript::{bitcoin, Descriptor, DescriptorPublicKey};
    use std::{env, fs, net, path, process, str::FromStr, thread, time};

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

        let desc_str = "wsh(andor(pk(03b506a1dbe57b4bf48c95e0c7d417b87dd3b4349d290d2e7e9ba72c912652d80a),older(10000),pk(0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce)))";
        let desc = Descriptor::<DescriptorPublicKey>::from_str(desc_str).unwrap();
        let config = Config {
            bitcoind_config: BitcoindConfig {
                network: bitcoin::Network::Bitcoin,
                cookie_path: path::PathBuf::new(),
                addr: net::SocketAddr::new(net::IpAddr::V4(net::Ipv4Addr::LOCALHOST), 0),
                poll_interval_secs: time::Duration::from_secs(1),
            },
            data_dir: Some(data_dir.clone()),
            daemon: None,
            log_level: log::LevelFilter::Debug,
            main_descriptor: desc,
        };

        let handle = DaemonHandle::start(config).unwrap();
        handle.shutdown();

        fs::remove_dir_all(&tmp_dir).unwrap();
    }
}
