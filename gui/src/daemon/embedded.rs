use std::sync::Mutex;

use super::{model::*, Daemon, DaemonError};
use minisafe::{config::Config, DaemonHandle};

pub struct EmbeddedDaemon {
    config: Config,
    handle: Option<Mutex<DaemonHandle>>,
}

impl EmbeddedDaemon {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            handle: None,
        }
    }

    pub fn start(&mut self) -> Result<(), DaemonError> {
        let handle = DaemonHandle::start_default(self.config.clone())
            .map_err(|e| DaemonError::Start(e.to_string()))?;
        self.handle = Some(Mutex::new(handle));
        Ok(())
    }
}

impl std::fmt::Debug for EmbeddedDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle").finish()
    }
}

impl Daemon for EmbeddedDaemon {
    fn is_external(&self) -> bool {
        false
    }

    fn load_config(&mut self, cfg: Config) -> Result<(), DaemonError> {
        if self.handle.is_none() {
            return Ok(());
        }

        let next =
            DaemonHandle::start_default(cfg).map_err(|e| DaemonError::Start(e.to_string()))?;
        self.handle.take().unwrap().into_inner().unwrap().shutdown();
        self.handle = Some(Mutex::new(next));
        Ok(())
    }

    fn config(&self) -> &Config {
        &self.config
    }

    fn stop(&mut self) -> Result<(), DaemonError> {
        if let Some(h) = self.handle.take() {
            let handle = h.into_inner().unwrap();
            handle.shutdown();
        }
        Ok(())
    }

    fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .lock()
            .unwrap()
            .control
            .get_info())
    }

    fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .lock()
            .unwrap()
            .control
            .get_new_address())
    }
}
