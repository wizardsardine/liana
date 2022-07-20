pub mod config;

use crate::config::{config_folder_path, Config};

use std::{error, fmt, fs, io, panic, path, process};

// A panic in any thread should stop the main thread, and print the panic.
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
        }
    }
}

impl error::Error for StartupError {}

impl From<io::Error> for StartupError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
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
        setup_panic_hook();

        // First, check the data directory
        let mut data_dir = config
            .data_dir
            .unwrap_or(config_folder_path().ok_or(StartupError::DefaultDataDirNotFound)?);
        data_dir.push(config.bitcoind_config.network.to_string());
        if !data_dir.as_path().exists() {
            create_datadir(&data_dir)?;
            log::info!("Created a new data directory at '{}'", data_dir.display());
        }

        Ok(Self {})
    }

    // NOTE: this moves out the data as it should not be reused after shutdown
    /// Shut down the Minisafe daemon.
    pub fn shutdown(self) {}
}
