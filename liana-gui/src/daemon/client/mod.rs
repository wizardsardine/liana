use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::iter::FromIterator;

use async_trait::async_trait;
use lianad::bip329::Labels;
use lianad::commands::{GetLabelsBip329Result, UpdateDerivIndexesResult};
use lianad::payjoin::types::PayjoinStatus;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info};

pub mod error;
pub mod jsonrpc;

use liana::miniscript::bitcoin::{
    address, bip32::ChildNumber, psbt::Psbt, Address, Network, OutPoint, Txid,
};
use lianad::{
    commands::{CoinStatus, CreateRecoveryResult, LabelItem},
    config::Config,
};

use super::{model::*, Daemon, DaemonBackend, DaemonError};
use crate::dir::LianaDirectory;

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

#[async_trait]
impl<C: Client + Send + Sync + Debug> Daemon for Lianad<C> {
    fn backend(&self) -> DaemonBackend {
        DaemonBackend::ExternalLianad
    }

    fn config(&self) -> Option<&Config> {
        None
    }

    async fn is_alive(
        &self,
        _datadir: &LianaDirectory,
        _network: Network,
    ) -> Result<(), DaemonError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), DaemonError> {
        Err(DaemonError::Unexpected(
            "GUI should not ask external client to stop".to_string(),
        ))
    }

    async fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        self.call("getinfo", Option::<Request>::None)
    }

    async fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        self.call("getnewaddress", Option::<Request>::None)
    }

    async fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, DaemonError> {
        self.call(
            "listrevealedaddresses",
            Some(vec![
                json!(is_change),
                json!(exclude_used),
                json!(limit),
                json!(start_index), // a `null` argument is parsed as `None` by the command
            ]),
        )
    }

    async fn receive_payjoin(&self) -> Result<GetAddressResult, DaemonError> {
        self.call("receivepayjoin", Option::<Request>::None)
    }

    async fn send_payjoin(&self, bip21: String, psbt: &Psbt) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value =
            self.call("sendpayjoin", Some(vec![bip21, psbt.to_string()]))?;
        Ok(())
    }

    async fn get_payjoin_info(&self, txid: &Txid) -> Result<PayjoinStatus, DaemonError> {
        self.call("getpayjoininfo", Some(vec![txid.to_string()]))
    }

    async fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError> {
        self.call("updatederivationindexes", Some(vec![receive, change]))
    }

    async fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<ListCoinsResult, DaemonError> {
        self.call(
            "listcoins",
            Some(vec![
                json!(statuses.iter().map(|s| s.to_arg()).collect::<Vec<&str>>()),
                json!(outpoints),
            ]),
        )
    }

    async fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        self.call("listspendtxs", Option::<Request>::None)
    }

    async fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, DaemonError> {
        let mut input = vec![
            json!(destinations),
            json!(coins_outpoints),
            json!(feerate_vb),
        ];
        if let Some(change_address) = change_address {
            input.push(json!(change_address));
        }
        self.call("createspend", Some(input))
    }

    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, DaemonError> {
        let mut input = vec![json!(txid.to_string()), json!(is_cancel)];
        if let Some(feerate_vb) = feerate_vb {
            input.push(json!(feerate_vb));
        }
        self.call("rbfpsbt", Some(input))
    }

    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        let spend_tx = psbt.to_string();
        let _res: serde_json::value::Value = self.call("updatespend", Some(vec![spend_tx]))?;
        Ok(())
    }

    async fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value =
            self.call("delspendtx", Some(vec![txid.to_string()]))?;
        Ok(())
    }

    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value =
            self.call("broadcastspend", Some(vec![txid.to_string()]))?;
        Ok(())
    }

    async fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        let _res: serde_json::value::Value = self.call("startrescan", Some(vec![t]))?;
        Ok(())
    }

    async fn list_confirmed_txs(
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

    async fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        self.call("listtransactions", Some(vec![txids]))
    }

    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        coins_outpoints: &[OutPoint],
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        let mut params = serde_json::Map::new();
        params.insert("address".to_string(), json!(address));
        params.insert("outpoints".to_string(), json!(coins_outpoints));
        params.insert("feerate".to_string(), json!(feerate_vb));
        if let Some(sequence) = sequence {
            params.insert("timelock".to_string(), json!(sequence));
        }
        let res: CreateRecoveryResult = self.call("createrecovery", Some(params))?;
        Ok(res.psbt)
    }

    async fn get_labels(
        &self,
        items: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        #[allow(unused_mut)]
        let mut items = items.iter().map(|a| a.to_string()).collect::<Vec<String>>();

        #[cfg(test)]
        items.sort();

        let res: GetLabelsResult = self.call("getlabels", Some(vec![items]))?;
        Ok(res.labels)
    }

    async fn update_labels(
        &self,
        items: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        let labels: HashMap<String, Option<String>> =
            HashMap::from_iter(items.iter().map(|(a, l)| (a.to_string(), l.clone())));
        let _res: serde_json::value::Value = self.call("updatelabels", Some(vec![labels]))?;
        Ok(())
    }

    async fn get_labels_bip329(&self, offset: u32, limit: u32) -> Result<Labels, DaemonError> {
        let res: GetLabelsBip329Result =
            self.call("getlabelsbip329", Some(vec![json!(offset), json!(limit)]))?;
        Ok(res.labels)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {}
