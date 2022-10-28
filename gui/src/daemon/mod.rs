pub mod client;
pub mod embedded;
pub mod model;

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::ErrorKind;

use minisafe::{
    config::Config,
    miniscript::bitcoin::{util::psbt::Psbt, Address, OutPoint, Txid},
};

#[derive(Debug, Clone)]
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
    Start(String),
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
                    .map(|c| c.clone())
                    .collect();
                model::SpendTx::new(tx.psbt, tx.change_index.map(|i| i as usize), coins)
            })
            .collect())
    }

    fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address, u64>,
        feerate_vb: u64,
    ) -> Result<model::CreateSpendResult, DaemonError>;

    fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError>;
    fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
    fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
}
