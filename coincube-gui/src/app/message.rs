use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    psbt::Psbt,
    Address, Txid,
};
use coincubed::config::Config as DaemonConfig;

use crate::{
    app::{
        breez::BreezError,
        cache::{DaemonCache, FiatPrice},
        error::Error,
        view,
        wallet::Wallet,
    },
    daemon::model::*,
    export::ImportExportMessage,
    hw::HardwareWalletMessage,
    node::bitcoind::Bitcoind,
    services::{
        coincube::{DownloadStats, TimeseriesPoint},
        fiat::{
            api::{ListCurrenciesResult, PriceApiError},
            PriceSource,
        },
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
    ReceiveAddress(Result<(Address, ChildNumber), Error>),
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
    /// Extension of payments for pagination.
    /// Tuple contains (Vec<Payment>, u64) where the u64 is the actual page limit used
    /// for fetching payments. This limit may differ from HISTORY_EVENT_PAGE_SIZE when
    /// multiple events occur in the same block, and is used to accurately detect the last page.
    PaymentsExtension(Result<(Vec<Payment>, u64), Error>),
    Payment(Result<(HistoryTransaction, usize), Error>),
    LabelsUpdated(Result<HashMap<String, Option<String>>, Error>),
    BroadcastModal(Result<HashSet<Txid>, Error>),
    RbfModal(Box<HistoryTransaction>, bool, Result<HashSet<Txid>, Error>),
    Export(ImportExportMessage),
    PaymentsLoaded(Result<Vec<breez_sdk_liquid::prelude::Payment>, BreezError>),
    RefundablesLoaded(Result<Vec<breez_sdk_liquid::prelude::RefundableSwap>, BreezError>),
    RefundCompleted(Result<breez_sdk_liquid::model::RefundResponse, BreezError>),
    BreezInfo(Result<breez_sdk_liquid::prelude::GetInfoResponse, BreezError>),
    BreezEvent(breez_sdk_liquid::prelude::SdkEvent),
    SettingsSaved,
    SettingsSaveFailed(Error),
    /// Store the Bitcoind handle produced by configure_and_start_internal_bitcoind so
    /// that its LockFile is kept alive for the lifetime of the App.
    SetInternalBitcoind(Bitcoind),
    /// Fired by the bitcoind-sync subscription to trigger a progress probe.
    PollBitcoindSync,
    /// Result of polling the pending local bitcoind's IBD sync progress.
    /// Carries `(verificationprogress, initialblockdownload)`.
    BitcoindSyncProgress(Result<(f64, bool), String>),
    /// Latest UpdateTip/blockheaders line streamed from the pending internal
    /// bitcoind's debug.log.  `None` means no matching line found yet.
    PendingBitcoindLog(Option<String>),
    InstallStats(InstallStatsMessage),
}

#[derive(Debug)]
pub enum InstallStatsMessage {
    /// u64 is the fetch generation — handlers ignore stale responses.
    DownloadStatsLoaded(u64, Result<DownloadStats, String>),
    TodayStatsLoaded(u64, Result<u32, String>),
    TimeseriesLoaded(u64, Result<Vec<TimeseriesPoint>, String>),
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
