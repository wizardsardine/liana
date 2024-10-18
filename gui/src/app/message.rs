use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use liana::{
    config::Config as DaemonConfig,
    miniscript::bitcoin::{
        bip32::{ChildNumber, Fingerprint},
        psbt::Psbt,
        Address, Txid,
    },
};

use crate::{
    app::{cache::Cache, error::Error, view, wallet::Wallet},
    daemon::model::*,
    hw::HardwareWalletMessage,
};

#[derive(Debug)]
pub enum Message {
    Tick,
    UpdateCache(Result<Cache, Error>),
    UpdatePanelCache(/* is current panel */ bool, Result<Cache, Error>),
    View(view::Message),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
    LoadWallet(Wallet),
    Info(Result<GetInfoResult, Error>),
    ReceiveAddress(Result<(Address, ChildNumber), Error>),
    Coins(Result<Vec<Coin>, Error>),
    Labels(Result<HashMap<String, String>, Error>),
    SpendTxs(Result<Vec<SpendTx>, Error>),
    Psbt(Result<(Psbt, Vec<String>), Error>),
    RbfPsbt(Result<Txid, Error>),
    Recovery(Result<SpendTx, Error>),
    Signed(Fingerprint, Result<Psbt, Error>),
    WalletUpdated(Result<Arc<Wallet>, Error>),
    Updated(Result<(), Error>),
    Saved(Result<(), Error>),
    Verified(Fingerprint, Result<(), Error>),
    StartRescan(Result<(), Error>),
    HardwareWallets(HardwareWalletMessage),
    HistoryTransactionsExtension(Result<Vec<HistoryTransaction>, Error>),
    HistoryTransactions(Result<Vec<HistoryTransaction>, Error>),
    PendingTransactions(Result<Vec<HistoryTransaction>, Error>),
    LabelsUpdated(Result<HashMap<String, Option<String>>, Error>),
    BroadcastModal(Result<HashSet<Txid>, Error>),
    RbfModal(Box<HistoryTransaction>, bool, Result<HashSet<Txid>, Error>),
}
