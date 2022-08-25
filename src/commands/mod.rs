//! # Minisafe commands
//!
//! External interface to the Minisafe daemon.

mod utils;

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, DatabaseInterface},
    DaemonControl, VERSION,
};
use utils::{deser_amount_from_sats, ser_amount};

use miniscript::{
    bitcoin,
    descriptor::{self, DescriptorTrait},
    TranslatePk2,
};
use serde::{Deserialize, Serialize};

impl DaemonControl {
    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        GetInfoResult {
            version: VERSION.to_string(),
            network: self.config.bitcoin_config.network,
            blockheight: self.bitcoin.chain_tip().height,
            sync: self.bitcoin.sync_progress(),
            descriptors: GetInfoDescriptors {
                main: self.config.main_descriptor.clone(),
            },
        }
    }

    /// Get a new deposit address. This will always generate a new deposit address, regardless of
    /// whether it was actually used.
    pub fn get_new_address(&self) -> GetAddressResult {
        let mut db_conn = self.db.connection();
        let index = db_conn.derivation_index();
        // TODO: handle should we wrap around instead of failing?
        db_conn.increment_derivation_index(&self.secp);
        let address = self
            .config
            .main_descriptor
            // TODO: have a descriptor newtype along with a derived descriptor one.
            .derive(index.into())
            .translate_pk2(|xpk| xpk.derive_public_key(&self.secp))
            .expect("All pubkeys were derived, no wildcard.")
            .address(self.config.bitcoin_config.network)
            .expect("It's a wsh() descriptor");
        GetAddressResult { address }
    }

    /// Get a list of all currently unspent coins.
    pub fn list_coins(&self) -> ListCoinsResult {
        let mut db_conn = self.db.connection();
        let coins: Vec<ListCoinsEntry> = db_conn
            .unspent_coins()
            // Can't use into_values as of Rust 1.48
            .into_iter()
            .map(|(_, coin)| {
                let Coin {
                    amount,
                    outpoint,
                    block_height,
                    ..
                } = coin;
                ListCoinsEntry {
                    amount,
                    outpoint,
                    block_height,
                }
            })
            .collect();
        ListCoinsResult { coins }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoDescriptors {
    pub main: descriptor::Descriptor<descriptor::DescriptorPublicKey>,
}

/// Information about the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoResult {
    pub version: String,
    pub network: bitcoin::Network,
    pub blockheight: i32,
    pub sync: f64,
    pub descriptors: GetInfoDescriptors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAddressResult {
    pub address: bitcoin::Address,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsEntry {
    #[serde(
        serialize_with = "ser_amount",
        deserialize_with = "deser_amount_from_sats"
    )]
    pub amount: bitcoin::Amount,
    pub outpoint: bitcoin::OutPoint,
    pub block_height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::*;
    use std::str::FromStr;

    #[test]
    fn getinfo() {
        let ms = DummyMinisafe::new();
        // We can query getinfo
        ms.handle.control.get_info();
        ms.shutdown();
    }

    #[test]
    fn getnewaddress() {
        let ms = DummyMinisafe::new();

        let control = &ms.handle.control;
        // We can get an address
        let addr = control.get_new_address().address;
        assert_eq!(
            addr,
            bitcoin::Address::from_str(
                "bc1qgudekhcrejgtlx3yhlvdul7t4q76e5lhm0vtcsndxs6aslh4r9jsqkqhwu"
            )
            .unwrap()
        );
        // We won't get the same twice.
        let addr2 = control.get_new_address().address;
        assert_ne!(addr, addr2);

        ms.shutdown();
    }
}
