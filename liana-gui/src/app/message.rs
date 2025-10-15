use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    psbt::Psbt,
    Address, Txid,
};
use lianad::config::Config as DaemonConfig;

use crate::{
    app::{
        cache::{DaemonCache, FiatPrice},
        error::Error,
        view,
        wallet::Wallet,
    },
    daemon::model::*,
    export::ImportExportMessage,
    hw::HardwareWalletMessage,
    services::fiat::{
        api::{ListCurrenciesResult, PriceApiError},
        PriceSource,
    },
};

#[derive(Debug)]
pub enum Message {
    Tick,
    UpdateDaemonCache(Result<DaemonCache, Error>),
    CacheUpdated,
    Fiat(FiatMessage),
    UpdatePanelCache(/* is current panel */ bool),
    View(view::Message),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
    LoadWallet(Wallet),
    Info(Result<GetInfoResult, Error>),
    ReceiveAddress(Result<(Address, ChildNumber, Option<String>), Error>),
    /// Revealed addresses. The second element contains the start index used for the request.
    RevealedAddresses(
        Result<ListRevealedAddressesResult, Error>,
        Option<ChildNumber>, // start_index
    ),
    Coins(Result<Vec<Coin>, Error>),
    /// When we want both coins and tip height together.
    CoinsTipHeight(Result<Vec<Coin>, Error>, Result<i32, Error>),
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
    Payments(Result<Vec<Payment>, Error>),
    PaymentsExtension(Result<Vec<Payment>, Error>),
    Payment(Result<(HistoryTransaction, usize), Error>),
    LabelsUpdated(Result<HashMap<String, Option<String>>, Error>),
    BroadcastModal(Result<HashSet<Txid>, Error>),
    RbfModal(Box<HistoryTransaction>, bool, Result<HashSet<Txid>, Error>),
    Export(ImportExportMessage),
    SendPayjoin(Result<(), Error>),
    PayjoinInitiated(Result<String, Error>),
}

impl From<ImportExportMessage> for Message {
    fn from(value: ImportExportMessage) -> Self {
        Message::View(view::Message::ImportExport(value))
    }
}

#[derive(Debug)]
pub enum FiatMessage {
    GetPriceResult(FiatPrice),
    ListCurrencies(PriceSource),
    ListCurrenciesResult(PriceSource, Result<ListCurrenciesResult, PriceApiError>),
    SaveChanges,
    ValidateCurrencySetting,
}

impl From<FiatMessage> for Message {
    fn from(value: FiatMessage) -> Self {
        Message::Fiat(value)
    }
}
