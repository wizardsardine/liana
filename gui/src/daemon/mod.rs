pub mod client;
pub mod embedded;
pub mod model;

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::ErrorKind;
use std::iter::FromIterator;

use liana::{
    commands::LabelItem,
    config::Config,
    miniscript::bitcoin::{address, psbt::Psbt, Address, OutPoint, Txid},
    StartupError,
};

#[derive(Debug)]
pub enum DaemonError {
    /// Something was wrong with the request.
    Rpc(i32, String),
    /// Something was wrong with the communication.
    Transport(Option<ErrorKind>, String),
    /// Something unexpected happened.
    Unexpected(String),
    /// No response.
    NoAnswer,
    /// Daemon stopped
    DaemonStopped,
    // Error at start up.
    Start(StartupError),
    // Error if the client is not supported.
    ClientNotSupported,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Rpc(code, e) => write!(f, "Daemon error rpc call: [{:?}] {}", code, e),
            Self::NoAnswer => write!(f, "Daemon returned no answer"),
            Self::DaemonStopped => write!(f, "Daemon stopped"),
            Self::Transport(kind, e) => write!(f, "Daemon transport error: [{:?}] {}", kind, e),
            Self::Unexpected(e) => write!(f, "Daemon unexpected error: {}", e),
            Self::Start(e) => write!(f, "Daemon did not start: {}", e),
            Self::ClientNotSupported => write!(f, "Daemon communication is not supported"),
        }
    }
}

pub trait Daemon: Debug {
    fn is_external(&self) -> bool;
    fn config(&self) -> Option<&Config>;
    fn stop(&self);
    fn get_info(&self) -> Result<model::GetInfoResult, DaemonError>;
    fn get_new_address(&self) -> Result<model::GetAddressResult, DaemonError>;
    fn list_coins(&self) -> Result<model::ListCoinsResult, DaemonError>;
    fn list_spend_txs(&self) -> Result<model::ListSpendResult, DaemonError>;
    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
    ) -> Result<model::CreateSpendResult, DaemonError>;
    fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError>;
    fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
    fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
    fn start_rescan(&self, t: u32) -> Result<(), DaemonError>;
    fn list_confirmed_txs(
        &self,
        _start: u32,
        _end: u32,
        _limit: u64,
    ) -> Result<model::ListTransactionsResult, DaemonError>;
    fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError>;
    fn list_txs(&self, txid: &[Txid]) -> Result<model::ListTransactionsResult, DaemonError>;
    fn get_labels(
        &self,
        labels: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError>;
    fn update_labels(&self, labels: &HashMap<LabelItem, Option<String>>)
        -> Result<(), DaemonError>;

    fn list_spend_transactions(&self) -> Result<Vec<model::SpendTx>, DaemonError> {
        let info = self.get_info()?;
        let coins = self.list_coins()?.coins;
        let mut spend_txs = Vec::new();
        for tx in self.list_spend_txs()?.spend_txs {
            let coins = coins
                .iter()
                .filter(|coin| {
                    tx.psbt
                        .unsigned_tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                })
                .cloned()
                .collect();

            spend_txs.push(model::SpendTx::new(
                tx.updated_at,
                tx.psbt,
                coins,
                &info.descriptors.main,
                info.network,
            ));
        }
        load_labels(self, &mut spend_txs)?;
        spend_txs.sort_by(|a, b| {
            if a.status == b.status {
                // last updated first
                b.updated_at.cmp(&a.updated_at)
            } else {
                // follows status enum order
                a.status.cmp(&b.status)
            }
        });
        Ok(spend_txs)
    }

    fn list_history_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let info = self.get_info()?;
        let coins = self.list_coins()?.coins;
        let txs = self.list_confirmed_txs(start, end, limit)?.transactions;
        let mut txs = txs
            .into_iter()
            .map(|tx| {
                let mut tx_coins = Vec::new();
                let mut change_indexes = Vec::new();
                for coin in &coins {
                    if coin.outpoint.txid == tx.tx.txid() {
                        change_indexes.push(coin.outpoint.vout as usize)
                    } else if tx
                        .tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                    {
                        tx_coins.push(coin.clone());
                    }
                }
                model::HistoryTransaction::new(
                    tx.tx,
                    tx.height,
                    tx.time,
                    tx_coins,
                    change_indexes,
                    info.network,
                )
            })
            .collect();
        load_labels(self, &mut txs)?;
        Ok(txs)
    }

    fn list_pending_txs(&self) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let info = self.get_info()?;
        let coins = self.list_coins()?.coins;
        let mut txids: Vec<Txid> = Vec::new();
        for coin in &coins {
            if coin.block_height.is_none() && !txids.contains(&coin.outpoint.txid) {
                txids.push(coin.outpoint.txid);
            }

            if let Some(spend) = coin.spend_info {
                if spend.height.is_none() && !txids.contains(&spend.txid) {
                    txids.push(spend.txid);
                }
            }
        }

        let txs = self.list_txs(&txids)?.transactions;
        let mut txs = txs
            .into_iter()
            .map(|tx| {
                let mut tx_coins = Vec::new();
                let mut change_indexes = Vec::new();
                for coin in &coins {
                    if coin.outpoint.txid == tx.tx.txid() {
                        change_indexes.push(coin.outpoint.vout as usize)
                    } else if tx
                        .tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                    {
                        tx_coins.push(coin.clone());
                    }
                }
                model::HistoryTransaction::new(
                    tx.tx,
                    tx.height,
                    tx.time,
                    tx_coins,
                    change_indexes,
                    info.network,
                )
            })
            .collect();

        load_labels(self, &mut txs)?;
        Ok(txs)
    }
}

fn load_labels<T: model::Labelled, D: Daemon + ?Sized>(
    daemon: &D,
    targets: &mut Vec<T>,
) -> Result<(), DaemonError> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut items = HashSet::<LabelItem>::new();
    for target in &*targets {
        for item in target.labelled() {
            items.insert(item);
        }
    }
    let labels = HashMap::from_iter(
        daemon
            .get_labels(&items)?
            .into_iter()
            .map(|(k, v)| (k, Some(v))),
    );
    for target in targets {
        target.load_labels(&labels);
    }
    Ok(())
}
