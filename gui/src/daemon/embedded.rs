use std::collections::HashMap;
use std::sync::RwLock;

use super::{model::*, Daemon, DaemonError};
use liana::{
    config::Config,
    miniscript::bitcoin::{util::psbt::Psbt, Address, OutPoint, Txid},
    DaemonHandle,
};

pub struct EmbeddedDaemon {
    config: Config,
    handle: Option<RwLock<DaemonHandle>>,
}

impl EmbeddedDaemon {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            handle: None,
        }
    }

    pub fn start(&mut self) -> Result<(), DaemonError> {
        let handle =
            DaemonHandle::start_default(self.config.clone()).map_err(DaemonError::Start)?;
        self.handle = Some(RwLock::new(handle));
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

        let next = DaemonHandle::start_default(cfg).map_err(DaemonError::Start)?;
        self.handle.take().unwrap().into_inner().unwrap().shutdown();
        self.handle = Some(RwLock::new(next));
        Ok(())
    }

    fn config(&self) -> Option<&Config> {
        Some(&self.config)
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
            .read()
            .unwrap()
            .control
            .get_info())
    }

    fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .get_new_address())
    }

    fn list_coins(&self) -> Result<ListCoinsResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .list_coins())
    }

    fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .list_spend())
    }

    fn list_confirmed_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .list_confirmed_transactions(start, end, limit))
    }

    fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        Ok(self
            .handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .list_transactions(txids))
    }

    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address, u64>,
        feerate_vb: u64,
    ) -> Result<CreateSpendResult, DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .create_spend(destinations, coins_outpoints, feerate_vb)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .update_spend(psbt.clone())
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .delete_spend(txid);
        Ok(())
    }

    fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .broadcast_spend(txid)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn start_rescan(&self, t: u32) -> Result<(), DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .start_rescan(t)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
    }

    fn create_recovery(&self, address: Address, feerate_vb: u64) -> Result<Psbt, DaemonError> {
        self.handle
            .as_ref()
            .ok_or(DaemonError::NoAnswer)?
            .read()
            .unwrap()
            .control
            .create_recovery(address, feerate_vb)
            .map_err(|e| DaemonError::Unexpected(e.to_string()))
            .map(|res| res.psbt)
    }
}
