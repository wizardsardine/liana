//! # Minisafe commands
//!
//! External interface to the Minisafe daemon.

use crate::{DaemonControl, VERSION};

use miniscript::{
    bitcoin,
    descriptor::{self, DescriptorTrait},
    TranslatePk2,
};

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

    /// Get a new deposit address. This will always generate a new deposit address, regardless of
    /// whether it was actually used.
    pub fn get_new_address(&self) -> bitcoin::Address {
        let mut db_conn = self.db.connection();
        let index = db_conn.derivation_index();
        // TODO: handle should we wrap around instead of failing?
        db_conn.update_derivation_index(index.increment().expect("TODO: handle wraparound"));
        self.config
            .main_descriptor
            // TODO: have a descriptor newtype along with a derived descriptor one.
            .derive(index.into())
            .translate_pk2(|xpk| xpk.derive_public_key(&self.secp))
            .expect("All pubkeys were derived, no wildcard.")
            .address(self.config.bitcoind_config.network)
            .expect("It's a wsh() descriptor")
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
