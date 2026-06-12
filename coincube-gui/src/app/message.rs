use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use zeroize::Zeroizing;

use coincube_core::miniscript::bitcoin::{
    address::NetworkUnchecked,
    bip32::{ChildNumber, Fingerprint},
    psbt::Psbt,
    Address, Txid,
};
use coincubed::config::Config as DaemonConfig;

use crate::{
    app::{
        breez_liquid::BreezError,
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
    /// Terminal no-op for the fire-and-forget vault recovery heartbeat
    /// (Estate Notifications — PR 2). The heartbeat POST must never block
    /// or affect sync, so its result is discarded here. Carries the result
    /// only so transient failures can be logged.
    RecoveryHeartbeatSent(Result<(), String>),
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
    /// Debounced trigger to run the create-spend "redraft" (coin selection)
    /// off the UI thread. Carries the redraft generation token; the handler
    /// ignores it if a newer edit has since superseded this generation.
    RedraftDebounce(u64),
    /// Result of an async redraft. Applied only if `seq` still matches the
    /// current redraft generation (otherwise it is a stale result from an
    /// edit the user has already typed past).
    RedraftResult {
        seq: u64,
        max_address: Address<NetworkUnchecked>,
        recipient_with_max: Option<usize>,
        destinations_is_empty: bool,
        result: Result<CreateSpendResult, Error>,
    },
    RbfPsbt(Result<Txid, Error>),
    Recovery(Result<SpendTx, Error>),
    Signed(Fingerprint, Result<Psbt, Error>),
    WalletUpdated(Result<Arc<Wallet>, Error>),
    Updated(Result<(), Error>),
    Saved(Result<(), Error>),
    Verified(Fingerprint, Result<(), Error>),
    StartRescan(Result<(), Error>),
    HardwareWallets(HardwareWalletMessage),
    /// Vault transactions next-page fetch result.
    /// Tuple: `(page_txs, server_exhausted)` where `server_exhausted` is
    /// `true` when the daemon returned fewer rows than the requested limit
    /// (i.e. end of history). It is measured on the raw response, before
    /// the inclusive-cursor overlap is deduplicated.
    HistoryTransactionsExtension(Result<(Vec<HistoryTransaction>, bool), Error>),
    /// Initial Vault transactions page-0 fetch result. The `u64` is the
    /// reload-generation token: the panel discards any response whose
    /// token isn't the latest, so a superseded reload can't overwrite
    /// fresher state. The inner tuple is `(pending_txs, page_0_confirmed_txs)`.
    HistoryTransactions(
        u64,
        Result<(Vec<HistoryTransaction>, Vec<HistoryTransaction>), Error>,
    ),
    /// Vault transactions background-refresh result dispatched from
    /// `Message::Tick`. Carries the same payload as `HistoryTransactions`,
    /// but the handler does NOT clear the currently displayed rows on
    /// dispatch and only overwrites `page_cache` when the user is still
    /// on page 0 with no NextPage fetch in flight, so a concurrent
    /// pagination action can't be stomped by a stale background reply.
    /// Errors are logged and swallowed — a silent retry on the next Tick
    /// is preferable to flashing an error banner over an otherwise-usable
    /// view.
    BackgroundHistoryTransactions(
        u64,
        Result<(Vec<HistoryTransaction>, Vec<HistoryTransaction>), Error>,
    ),
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
    /// Liquid transactions page fetch result. The `u64` is the
    /// fetch-generation token: the panel discards any response whose token
    /// is not the latest dispatched, so a stale pagination response can't
    /// overwrite data fetched by a subsequent reload / filter change.
    PaymentsLoaded(
        u64,
        Result<Vec<crate::app::wallets::DomainPayment>, BreezError>,
    ),
    RefundablesLoaded(Result<Vec<crate::app::wallets::DomainRefundableSwap>, BreezError>),
    /// Result of a debounced background poll started by
    /// `App::refresh_refundables_task`. Distinct from `RefundablesLoaded`
    /// (which is produced by manual panel reloads) so that only poll
    /// responses touch the App's debounce/in-flight tracking. A reload
    /// response racing ahead of a poll must not clear the in-flight flag,
    /// or a second concurrent `list_refundables()` could be launched.
    RefundablesPolled(Result<Vec<crate::app::wallets::DomainRefundableSwap>, BreezError>),
    /// Result of a user-initiated `refund_onchain_tx` call. The `swap_address`
    /// is carried alongside the response so the handler can look up the exact
    /// `in_flight_refunds` entry that originated this refund — necessary when
    /// more than one refund is in flight, since the SDK response itself does
    /// not identify the originating swap.
    RefundCompleted {
        swap_address: String,
        result: Result<breez_sdk_liquid::model::RefundResponse, BreezError>,
    },
    BreezInfo(Result<breez_sdk_liquid::prelude::GetInfoResponse, BreezError>),
    BreezEvent(breez_sdk_liquid::prelude::SdkEvent),
    /// Forwarded from the [`coincube-spark-bridge`] subprocess via
    /// `SparkBackend::event_subscription`. Wrapped in the
    /// [`crate::app::breez_spark::SparkClientEvent`] newtype so the
    /// app-level message doesn't depend on `coincube_spark_protocol`
    /// directly.
    SparkEvent(crate::app::breez_spark::SparkClientEvent),
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
    /// Connect realtime stream event forwarded from the gRPC subscription.
    /// `None` payload means "no grpc_url available; stream stays offline".
    ConnectStreamReady(Option<crate::services::connect::grpc::stream::ConnectStreamConfig>),
    /// One message from the live Connect realtime stream subscription.
    /// Routed to the active vault PSBT modal in PR B; PR A logs only.
    ConnectStream(crate::services::connect::grpc::ConnectStreamMessage),
    /// Routed to the open `KeychainSignModal` on the active PSBT, when
    /// any. Wraps the modal's internal lifecycle messages so they can
    /// be dispatched through Iced's top-level update loop.
    KeychainSign(crate::app::state::vault::keychain_sign::KeychainSignMessage),
    /// Fired by the in-app Connect panel right after a successful login
    /// (REST OTP-verify or password). Carries the freshly-issued JWTs so
    /// the App can persist them to `connect.json`, register a signer
    /// device via gRPC `RegisterDevice`, and bootstrap the realtime
    /// stream — work that the home path does via
    /// `register_signer_device_best_effort` + `connect_stream_ready_task`
    /// at app-init time, but which the runtime in-app login flow
    /// previously skipped. See PLAN comment near `mod.rs:2374`.
    InAppConnectLoginCompleted {
        token: String,
        refresh_token: String,
        email: String,
    },
    /// Internal: chains from `InAppConnectLoginCompleted` once
    /// tokens have been persisted and a device_id registered. The
    /// payload carries everything `connect_stream_ready_task` needs
    /// so we can fire it from a non-init context.
    TriggerConnectStreamReady {
        network: coincube_core::miniscript::bitcoin::Network,
        datadir: crate::dir::CoincubeDirectory,
        tokens: std::sync::Arc<
            tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>,
        >,
        email: String,
        cube_uuid: Option<String>,
    },
    /// Persist a completed duress enrollment (Phases 2 & 8). Emitted by the
    /// Connect panel, which lacks Cube/datadir context; handled by the App,
    /// which writes the active Cube's duress PIN hash and this device's
    /// encrypted duress code into `DuressLocalState`.
    CompleteDuressEnrollment(DuressEnrollmentPayload),
}

/// Sensitive payload for [`Message::CompleteDuressEnrollment`]. The duress PIN
/// and code are wrapped in `Zeroizing` so their heap bytes are scrubbed on
/// drop (the message is cloned/relayed across the App/Home/Launcher surfaces).
/// `Debug` is also hand-written to redact them so they never reach a tracing
/// snapshot of the parent message. `Clone` is required because the Home/Launcher
/// `Message` enums (which relay this from their Connect panels) derive `Clone`.
#[derive(Clone)]
pub struct DuressEnrollmentPayload {
    /// The user's re-typed regular PIN. Carried so the persist step can verify
    /// it against each Cube's ACTUAL stored PIN (the wizard can only check the
    /// re-typed value) before arming the duress PIN.
    pub regular_pin: Zeroizing<String>,
    pub duress_pin: Zeroizing<String>,
    pub duress_code: Zeroizing<String>,
    /// Connect account id, persisted so the unauth activation POST can address
    /// it later. `None` for sovereign (no-Connect) enrollment.
    pub account_id: Option<String>,
    pub gen: u64,
}

impl std::fmt::Debug for DuressEnrollmentPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuressEnrollmentPayload")
            .field("regular_pin", &"<redacted>")
            .field("duress_pin", &"<redacted>")
            .field("duress_code", &"<redacted>")
            .field("gen", &self.gen)
            .finish()
    }
}

#[derive(Debug)]
pub enum InstallStatsMessage {
    /// u64 is the fetch generation — handlers ignore stale responses.
    DownloadStatsLoaded(u64, Result<DownloadStats, String>),
    TodayStatsLoaded(u64, Result<u32, String>),
    TimeseriesLoaded(
        u64,
        crate::services::coincube::StatsPeriod,
        Result<Vec<TimeseriesPoint>, String>,
    ),
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
