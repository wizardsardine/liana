pub mod client;
pub mod embedded;
pub mod model;

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::ErrorKind;

use liana::{
    config::Config,
    miniscript::bitcoin::{util::psbt::Psbt, Address, OutPoint, Txid},
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
    // Error at start up.
    Start(StartupError),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Rpc(code, e) => write!(f, "Daemon error rpc call: [{:?}] {}", code, e),
            Self::NoAnswer => write!(f, "Daemon returned no answer"),
            Self::Transport(kind, e) => write!(f, "Daemon transport error: [{:?}] {}", kind, e),
            Self::Unexpected(e) => write!(f, "Daemon unexpected error: {}", e),
            Self::Start(e) => write!(f, "Daemon did not start: {}", e),
        }
    }
}

pub trait Daemon: Debug {
    fn is_external(&self) -> bool;
    fn load_config(&mut self, _cfg: Config) -> Result<(), DaemonError> {
        Ok(())
    }
    fn config(&self) -> &Config;
    fn stop(&mut self) -> Result<(), DaemonError>;
    fn get_info(&self) -> Result<model::GetInfoResult, DaemonError>;
    fn get_new_address(&self) -> Result<model::GetAddressResult, DaemonError>;
    fn list_coins(&self) -> Result<model::ListCoinsResult, DaemonError>;
    fn list_spend_txs(&self) -> Result<model::ListSpendResult, DaemonError>;
    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address, u64>,
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
    fn create_recovery(&self, address: Address, feerate_vb: u64) -> Result<Psbt, DaemonError>;
    fn list_txs(&self, txid: &[Txid]) -> Result<model::ListTransactionsResult, DaemonError>;

    fn list_spend_transactions(&self) -> Result<Vec<model::SpendTx>, DaemonError> {
        let coins = self.list_coins()?.coins;
        let spend_txs = self.list_spend_txs()?.spend_txs;
        Ok(spend_txs
            .into_iter()
            .map(|tx| {
                let coins = coins
                    .iter()
                    .filter(|coin| {
                        tx.psbt
                            .unsigned_tx
                            .input
                            .iter()
                            .any(|input| input.previous_output == coin.outpoint)
                    })
                    .copied()
                    .collect();
                model::SpendTx::new(tx.psbt, tx.change_index.map(|i| i as usize), coins)
            })
            .collect())
    }

    fn list_history_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let coins = self.list_coins()?.coins;
        let txs = self.list_confirmed_txs(start, end, limit)?.transactions;
        Ok(txs
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
                        tx_coins.push(*coin);
                    }
                }
                model::HistoryTransaction::new(tx.tx, tx.height, tx.time, tx_coins, change_indexes)
            })
            .collect())
    }

    fn list_pending_txs(&self) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let coins = self.list_coins()?.coins;
        let mut txids: Vec<Txid> = Vec::new();
        for coin in &coins {
            if let Some(spend) = coin.spend_info {
                if spend.height.is_none() && !txids.contains(&spend.txid) {
                    txids.push(spend.txid);
                }
            }
        }

        let txs = self.list_txs(&txids)?.transactions;
        Ok(txs
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
                        tx_coins.push(*coin);
                    }
                }
                model::HistoryTransaction::new(tx.tx, tx.height, tx.time, tx_coins, change_indexes)
            })
            .collect())
    }
}
