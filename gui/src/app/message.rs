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
    app::{error::Error, view, wallet::Wallet},
    daemon::model::*,
    hw::HardwareWalletMessage,
};

#[derive(Debug)]
pub enum Message {
    Tick,
    View(view::Message),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
    LoadWallet,
    WalletLoaded(Result<Arc<Wallet>, Error>),
    Info(Result<GetInfoResult, Error>),
    ReceiveAddress(Result<(Address, ChildNumber), Error>),
    Coins(Result<Vec<Coin>, Error>),
    Labels(Result<HashMap<String, String>, Error>),
    SpendTxs(Result<Vec<SpendTx>, Error>),
    Psbt(Result<Psbt, Error>),
    RbfPsbt(Result<Txid, Error>),
    Recovery(Result<SpendTx, Error>),
    Signed(Fingerprint, Result<Psbt, Error>),
    WalletRegistered(Result<Fingerprint, Error>),
    Updated(Result<(), Error>),
    Saved(Result<(), Error>),
    Verified(Fingerprint, Result<(), Error>),
    StartRescan(Result<(), Error>),
    HardwareWallets(HardwareWalletMessage),
    HistoryTransactions(Result<Vec<HistoryTransaction>, Error>),
    PendingTransactions(Result<Vec<HistoryTransaction>, Error>),
    LabelsUpdated(Result<HashMap<String, Option<String>>, Error>),
    BroadcastModal(Result<HashSet<Txid>, Error>),
    RbfModal(HistoryTransaction, bool, Result<HashSet<Txid>, Error>),
}
