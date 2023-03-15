use std::sync::Arc;

use liana::{
    config::Config as DaemonConfig,
    miniscript::bitcoin::{
        util::{bip32::Fingerprint, psbt::Psbt},
        Address,
    },
};

use crate::{
    app::{error::Error, view, wallet::Wallet},
    daemon::model::*,
    hw::HardwareWallet,
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
    ReceiveAddress(Result<Address, Error>),
    Coins(Result<Vec<Coin>, Error>),
    SpendTxs(Result<Vec<SpendTx>, Error>),
    Psbt(Result<Psbt, Error>),
    Recovery(Result<SpendTx, Error>),
    Signed(Result<(Psbt, Fingerprint), Error>),
    WalletRegistered(Result<Fingerprint, Error>),
    Updated(Result<(), Error>),
    Saved(Result<(), Error>),
    StartRescan(Result<(), Error>),
    ConnectedHardwareWallets(Vec<HardwareWallet>),
    HistoryTransactions(Result<Vec<HistoryTransaction>, Error>),
    PendingTransactions(Result<Vec<HistoryTransaction>, Error>),
}
