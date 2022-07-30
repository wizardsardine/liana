//! # Minisafe commands
//!
//! External interface to the Minisafe daemon.

use crate::{DaemonControl, VERSION};

use miniscript::{bitcoin, descriptor};

impl DaemonControl {
    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        GetInfoResult {
            version: VERSION.to_string(),
            network: self.config.bitcoind_config.network,
            blockheight: self.bitcoin.chain_tip().height,
            sync: self.bitcoin.sync_progress(),
            descriptors: GetInfoDescriptors {
                main: self.config.main_descriptor.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct GetInfoDescriptors {
    pub main: descriptor::Descriptor<descriptor::DescriptorPublicKey>,
}

/// Information about the daemon
#[derive(Debug, Clone)]
pub struct GetInfoResult {
    pub version: String,
    pub network: bitcoin::Network,
    pub blockheight: i32,
    pub sync: f64,
    pub descriptors: GetInfoDescriptors,
}
