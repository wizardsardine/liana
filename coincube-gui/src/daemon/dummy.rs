use crate::daemon::DaemonError;
use coincube_core::miniscript::bitcoin::{
    address, bip32::ChildNumber, psbt::Psbt, Address, Network, OutPoint, Txid,
};
use coincubed::bip329::Labels;
use coincubed::commands::{CoinStatus, LabelItem, UpdateDerivIndexesResult};
use coincubed::config::Config;
use std::collections::{HashMap, HashSet};

// Ensure this struct is visible where you need it
#[derive(Debug, Clone)]
pub struct DummyDaemon;

#[async_trait::async_trait]
impl super::Daemon for DummyDaemon {
    fn backend(&self) -> super::DaemonBackend {
        // Return a variant that makes sense for a dummy, usually Remote or External
        super::DaemonBackend::RemoteBackend
    }

    fn config(&self) -> Option<&Config> {
        None
    }

    async fn is_alive(
        &self,
        _datadir: &crate::dir::CoincubeDirectory,
        _network: Network,
    ) -> Result<(), DaemonError> {
        // You might want this to return Ok(()) to simulate a running daemon
        Ok(())
    }

    async fn stop(&self) -> Result<(), DaemonError> {
        Ok(())
    }

    async fn get_info(&self) -> Result<super::model::GetInfoResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn get_new_address(&self) -> Result<super::model::GetAddressResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn list_revealed_addresses(
        &self,
        _is_change: bool,
        _exclude_used: bool,
        _limit: usize,
        _start_index: Option<ChildNumber>,
    ) -> Result<super::model::ListRevealedAddressesResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn update_deriv_indexes(
        &self,
        _receive: Option<u32>,
        _change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn list_coins(
        &self,
        _statuses: &[CoinStatus],
        _outpoints: &[OutPoint],
    ) -> Result<super::model::ListCoinsResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn list_spend_txs(&self) -> Result<super::model::ListSpendResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn create_spend_tx(
        &self,
        _coins_outpoints: &[OutPoint],
        _destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        _feerate_vb: u64,
        _change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<super::model::CreateSpendResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn rbf_psbt(
        &self,
        _txid: &Txid,
        _is_cancel: bool,
        _feerate_vb: Option<u64>,
    ) -> Result<super::model::CreateSpendResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn update_spend_tx(&self, _psbt: &Psbt) -> Result<(), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn delete_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn broadcast_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn start_rescan(&self, _t: u32) -> Result<(), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn list_confirmed_txs(
        &self,
        _start: u32,
        _end: u32,
        _limit: u64,
    ) -> Result<super::model::ListTransactionsResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn create_recovery(
        &self,
        _address: Address<address::NetworkUnchecked>,
        _coins_outpoints: &[OutPoint],
        _feerate_vb: u64,
        _sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn list_txs(
        &self,
        _txid: &[Txid],
    ) -> Result<super::model::ListTransactionsResult, DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    async fn get_labels(
        &self,
        _labels: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        // Return empty map to prevent iteration errors if called
        Ok(HashMap::new())
    }

    async fn update_labels(
        &self,
        _labels: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        Ok(())
    }

    async fn get_labels_bip329(&self, _offset: u32, _limit: u32) -> Result<Labels, DaemonError> {
        Err(DaemonError::NotImplemented)
    }
}
