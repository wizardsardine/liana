use std::collections::{HashMap, HashSet};

use super::{model::*, Daemon, DaemonError};
use liana::{
    commands::{CommandError, LabelItem},
    config::Config,
    miniscript::bitcoin::{address, psbt::Psbt, Address, OutPoint, Txid},
    DaemonControl, DaemonHandle,
};

pub struct EmbeddedDaemon {
    config: Config,
    handle: DaemonHandle,
}

impl EmbeddedDaemon {
    pub fn start(config: Config) -> Result<EmbeddedDaemon, DaemonError> {
        let handle = DaemonHandle::start_default(config.clone()).map_err(DaemonError::Start)?;
        Ok(Self { handle, config })
    }

    fn control(&self) -> Result<&DaemonControl, DaemonError> {
        if self.handle.shutdown_complete() {
            Err(DaemonError::DaemonStopped)
        } else {
            Ok(&self.handle.control)
        }
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

    fn config(&self) -> Option<&Config> {
        Some(&self.config)
    }

    fn stop(&self) {
        self.handle.trigger_shutdown();
        while !self.handle.shutdown_complete() {
            tracing::debug!("Waiting daemon to shutdown");
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        Ok(self.control()?.get_info())
    }

    fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        Ok(self.control()?.get_new_address())
    }

    fn list_coins(&self) -> Result<ListCoinsResult, DaemonError> {
        Ok(self.control()?.list_coins(&[], &[]))
    }

    fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        Ok(self.control()?.list_spend())
    }

    fn list_confirmed_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        Ok(self
            .control()?
            .list_confirmed_transactions(start, end, limit))
    }

    fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        Ok(self.control()?.list_transactions(txids))
    }

    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.control()?
            .create_spend(destinations, coins_outpoints, feerate_vb, None)
            .map_err(|e| match e {
                CommandError::CoinSelectionError(_) => DaemonError::CoinSelectionError,
                e => DaemonError::Unexpected(e.to_string()),
            })
    }

    fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        self.control()?
            .update_spend(psbt.clone())
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.control()?.delete_spend(txid);
        Ok(())
    }

    fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.control()?
            .broadcast_spend(txid)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        self.control()?
            .start_rescan(t)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        self.control()?
            .create_recovery(address, feerate_vb, sequence)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
            .map(|res| res.psbt)
    }

    fn get_labels(
        &self,
        items: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        Ok(self.handle.control.get_labels(items).labels)
    }

    fn update_labels(&self, items: &HashMap<LabelItem, Option<String>>) -> Result<(), DaemonError> {
        self.handle.control.update_labels(items);
        Ok(())
    }
}
