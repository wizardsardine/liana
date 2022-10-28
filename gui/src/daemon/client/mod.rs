use std::collections::HashMap;
use std::fmt::Debug;

use log::{error, info};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub mod error;
pub mod jsonrpc;

use minisafe::{
    config::Config,
    miniscript::bitcoin::{consensus, util::psbt::Psbt, Address, OutPoint, Txid},
};

use super::{model::*, Daemon, DaemonError};

pub trait Client {
    type Error: Into<DaemonError> + Debug;
    fn request<S: Serialize + Debug, D: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<D, Self::Error>;
}

#[derive(Debug, Clone)]
pub struct Minisafed<C: Client> {
    config: Config,
    client: C,
}

impl<C: Client> Minisafed<C> {
    pub fn new(client: C, config: Config) -> Minisafed<C> {
        Minisafed { client, config }
    }

    /// Generic call function for RPC calls.
    fn call<T: Serialize + Debug, U: DeserializeOwned + Debug>(
        &self,
        method: &str,
        input: Option<T>,
    ) -> Result<U, DaemonError> {
        info!("{}", method);
        self.client.request(method, input).map_err(|e| {
            error!("method {} failed: {:?}", method, e);
            e.into()
        })
    }
}

impl<C: Client + Debug> Daemon for Minisafed<C> {
    fn is_external(&self) -> bool {
        true
    }

    fn config(&self) -> &Config {
        &self.config
    }

    fn stop(&mut self) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value = self.call("stop", Option::<Request>::None)?;
        Ok(())
    }

    fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        self.call("getinfo", Option::<Request>::None)
    }

    fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        self.call("getnewaddress", Option::<Request>::None)
    }

    fn list_coins(&self) -> Result<ListCoinsResult, DaemonError> {
        self.call("listcoins", Option::<Request>::None)
    }

    fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        self.call("listspend", Option::<Request>::None)
    }

    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address, u64>,
        feerate_vb: u64,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.call(
            "createspend",
            Some(vec![
                json!(coins_outpoints),
                json!(destinations),
                json!(feerate_vb),
            ]),
        )
    }

    fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        let spend_tx = base64::encode(&consensus::serialize(psbt));
        let _res: serde_json::value::Value = self.call("updatespend", Some(vec![spend_tx]))?;
        Ok(())
    }

    fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value =
            self.call("deletespend", Some(vec![txid.to_string()]))?;
        Ok(())
    }

    fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value =
            self.call("broadcastspend", Some(vec![txid.to_string()]))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {}
