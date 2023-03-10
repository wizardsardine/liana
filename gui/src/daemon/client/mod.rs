use std::collections::HashMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info};

pub mod error;
pub mod jsonrpc;

use liana::{
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
pub struct Lianad<C: Client> {
    client: C,
}

impl<C: Client> Lianad<C> {
    pub fn new(client: C) -> Lianad<C> {
        Lianad { client }
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

impl<C: Client + Debug> Daemon for Lianad<C> {
    fn is_external(&self) -> bool {
        true
    }

    fn config(&self) -> Option<&Config> {
        None
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
        self.call("listspendtxs", Option::<Request>::None)
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

    fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value = self.call("startrescan", Some(vec![t]))?;
        Ok(())
    }

    fn list_confirmed_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        self.call(
            "listconfirmed",
            Some(vec![json!(start), json!(end), json!(limit)]),
        )
    }

    fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        self.call("listtransactions", Some(vec![txids]))
    }

    fn create_recovery(&self, address: Address, feerate_vb: u64) -> Result<Psbt, DaemonError> {
        let res: CreateSpendResult = self.call(
            "createrecovery",
            Some(vec![json!(address), json!(feerate_vb)]),
        )?;
        Ok(res.psbt)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {}
