use lianad::commands::UpdateDerivIndexesResult;
use lianad::{bip329::Labels, payjoin::types::PayjoinStatus};
use std::collections::{HashMap, HashSet};
use tokio::sync::Mutex;

use super::{model::*, node, Daemon, DaemonBackend, DaemonError};
use crate::dir::LianaDirectory;
use async_trait::async_trait;
use liana::miniscript::bitcoin::{
    address, bip32::ChildNumber, psbt::Psbt, Address, Network, OutPoint, Txid,
};
use lianad::{
    commands::{CoinStatus, LabelItem},
    config::Config,
    DaemonControl, DaemonHandle,
};

pub struct EmbeddedDaemon {
    config: Config,
    handle: Mutex<Option<DaemonHandle>>,
}

impl EmbeddedDaemon {
    pub fn start(config: Config) -> Result<EmbeddedDaemon, DaemonError> {
        let handle =
            DaemonHandle::start_default(config.clone(), false).map_err(DaemonError::Start)?;
        Ok(Self {
            handle: Mutex::new(Some(handle)),
            config,
        })
    }

    pub async fn command<T, F>(&self, method: F) -> Result<T, DaemonError>
    where
        F: FnOnce(&mut DaemonControl) -> Result<T, DaemonError>,
    {
        match self.handle.lock().await.as_mut() {
            Some(DaemonHandle::Controller { control, .. }) => method(control),
            Some(_) => unreachable!("No lianad rpc server must be started"),
            None => Err(DaemonError::DaemonStopped),
        }
    }
}

impl<T> From<std::sync::PoisonError<T>> for DaemonError {
    fn from(value: std::sync::PoisonError<T>) -> Self {
        DaemonError::Unexpected(format!("Daemon panic: {}", value))
    }
}

impl std::fmt::Debug for EmbeddedDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle").finish()
    }
}

#[async_trait]
impl Daemon for EmbeddedDaemon {
    fn backend(&self) -> DaemonBackend {
        let node_type = self
            .config
            .bitcoin_backend
            .as_ref()
            .map(node::NodeType::from);
        DaemonBackend::EmbeddedLianad(node_type)
    }

    fn config(&self) -> Option<&Config> {
        Some(&self.config)
    }

    async fn is_alive(
        &self,
        _datadir: &LianaDirectory,
        _network: Network,
    ) -> Result<(), DaemonError> {
        let mut handle = self.handle.lock().await;
        if let Some(h) = handle.as_ref() {
            if h.is_alive() {
                return Ok(());
            }
        }
        // if the daemon poller is not alive, we try to terminate it to fetch the error.
        if let Some(h) = handle.take() {
            h.stop()
                .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), DaemonError> {
        let mut handle = self.handle.lock().await;
        if let Some(h) = handle.take() {
            h.stop()
                .map_err(|e| DaemonError::Unexpected(e.to_string()))?;
        }
        Ok(())
    }

    async fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        self.command(|daemon| Ok(daemon.get_info())).await
    }

    async fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        self.command(|daemon| Ok(daemon.get_new_address())).await
    }

    async fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .list_revealed_addresses(is_change, exclude_used, limit, start_index)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn receive_payjoin(&self) -> Result<GetAddressResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .receive_payjoin()
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn send_payjoin(&self, bip21: String, psbt: &Psbt) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .init_payjoin_sender(bip21, psbt)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn get_payjoin_info(&self, txid: &Txid) -> Result<PayjoinStatus, DaemonError> {
        self.command(|daemon| {
            daemon
                .get_payjoin_info(txid)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .update_deriv_indexes(receive, change)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<ListCoinsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_coins(statuses, outpoints)))
            .await
    }

    async fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .list_spend(None)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn list_confirmed_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_confirmed_transactions(start, end, limit)))
            .await
    }

    async fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        self.command(|daemon| Ok(daemon.list_transactions(txids)))
            .await
    }

    async fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .create_spend(destinations, coins_outpoints, feerate_vb, change_address)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.command(|daemon| {
            daemon
                .rbf_psbt(txid, is_cancel, feerate_vb)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .update_spend(psbt.clone())
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon.delete_spend(txid);
            Ok(())
        })
        .await
    }

    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .broadcast_spend(txid)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon
                .start_rescan(t)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        coins_outpoints: &[OutPoint],
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        self.command(|daemon| {
            daemon
                .create_recovery(address, coins_outpoints, feerate_vb, sequence)
                .map(|res| res.psbt)
                .map_err(|e| DaemonError::Unexpected(e.to_string()))
        })
        .await
    }

    async fn get_labels(
        &self,
        items: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        self.command(|daemon| Ok(daemon.get_labels(items).labels))
            .await
    }

    async fn update_labels(
        &self,
        items: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        self.command(|daemon| {
            daemon.update_labels(items);
            Ok(())
        })
        .await
    }

    async fn get_labels_bip329(&self, offset: u32, limit: u32) -> Result<Labels, DaemonError> {
        self.command(|daemon| Ok(daemon.get_labels_bip329(offset, limit).labels))
            .await
    }
}
