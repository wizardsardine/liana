use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use minisafe::config::Config as DaemonConfig;

use crate::{
    app::{config, error::Error, menu::Menu},
    conversion::Converter,
    daemon::Daemon,
};

/// Context is an object passing general information
/// and service clients through the application components.
pub struct Context {
    pub config: ConfigContext,
    pub blockheight: i32,
    pub daemon: Arc<dyn Daemon + Sync + Send>,
    pub converter: Converter,
    pub menu: Menu,
    pub managers_threshold: usize,
}

impl Context {
    pub fn new(
        config: ConfigContext,
        daemon: Arc<dyn Daemon + Sync + Send>,
        converter: Converter,
        menu: Menu,
    ) -> Self {
        Self {
            config,
            blockheight: 0,
            daemon,
            converter,
            menu,
            managers_threshold: 0,
        }
    }

    pub fn network(&self) -> bitcoin::Network {
        self.config.daemon.bitcoin_config.network
    }

    pub fn load_daemon_config(&mut self, cfg: DaemonConfig) -> Result<(), Error> {
        loop {
            if let Some(daemon) = Arc::get_mut(&mut self.daemon) {
                daemon.load_config(cfg)?;
                break;
            }
        }

        let mut daemon_config_file = OpenOptions::new()
            .write(true)
            .open(&self.config.gui.minisafed_config_path)
            .map_err(|e| Error::Config(e.to_string()))?;

        let content =
            toml::to_string(&self.config.daemon).map_err(|e| Error::Config(e.to_string()))?;

        daemon_config_file
            .write_all(content.as_bytes())
            .map_err(|e| {
                log::warn!("failed to write to file: {:?}", e);
                Error::Config(e.to_string())
            })?;

        Ok(())
    }
}

pub struct ConfigContext {
    pub daemon: DaemonConfig,
    pub gui: config::Config,
}
