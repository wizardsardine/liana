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
    /// A shown toast's expiry timer fired. Ignored unless the toast is still
    /// on screen and its deadline has passed (see `App::arm_toast_timers`).
    ExpireToast(usize),
    /// The Cube name typed in Settings was written to this device's Cube
    /// settings (`Ok(name)`) or could not be.
    CubeNameSaved(Result<String, String>),
    /// Connect accepted (`Ok`) or refused the renamed Cube. `name` is the
    /// name that was sent, so a reply to an older rename is not mistaken
    /// for the current one.
    CubeNameSynced {
        cube_id: String,
        name: String,
        result: Result<(), String>,
    },
    /// The Spark nickname typed in Settings was written to this device's
    /// Cube settings (`Ok`, `None` for plain "Spark") or could not be.
    /// `cube_id` is the Cube whose settings were written: a save that lands
    /// after the tab has opened another Cube is not this App's.
    SparkNicknameSaved {
        cube_id: String,
        result: Result<Option<String>, String>,
    },
    ActivityVaultLoaded(u64, Result<(Vec<HistoryTransaction>, bool), String>),
    ActivityPsbtLoaded(u64, Result<Vec<SpendTx>, String>),
    ActivitySparkLoaded(
        u64,
        Result<(Vec<coincube_spark_protocol::PaymentSummary>, bool), String>,
    ),
    /// A wallet page's quiet refresh of Spark's first page: it replaces the
    /// loaded rows instead of extending them like a "Load older" page.
    ActivitySparkRefreshed(
        u64,
        Result<(Vec<coincube_spark_protocol::PaymentSummary>, bool), String>,
    ),
    ActivityLiquidLoaded(
        u64,
        Result<(Vec<crate::app::wallets::DomainPayment>, bool), String>,
    ),
    ActivityRefundablesLoaded(
        u64,
        Result<Vec<crate::app::wallets::DomainRefundableSwap>, String>,
    ),
    ActivityOpenDetail(super::state::activity::ActivityTarget),
    /// A label save from Activity, by its request number: the entry id, the
    /// stored key and the label.
    ActivityLabelSaved(u64, Result<(String, String, Option<String>), String>),
    ImportedBitcoin(Box<super::state::imported_bitcoin::ImportEvent>),
    VaultSpendPrepared {
        generation: u64,
        result: Result<(Psbt, Vec<String>), Error>,
        identities: Vec<(usize, crate::services::branta::LookupResult)>,
    },
    Tick,
    /// Answer to the fork feature check `attempt` (see
    /// `App::fork_feature_check`); `enabled` is false when the feature is off.
    LocalForkFeatureChecked {
        attempt: u64,
        enabled: bool,
    },
    UpdateDaemonCache(Result<DaemonCache, Error>),
    CacheUpdated,
    /// Outcome of clearing a restored Vault's pending-rescan marker after a
    /// completed full scan: `Err` carries why the settings write failed, and the
    /// next refresh tries again.
    RescanObligationRetired(Result<(), String>),
    /// Terminal no-op for the fire-and-forget vault recovery heartbeat
    /// (Estate Notifications — PR 2). The heartbeat POST must never block
    /// or affect sync, so its result is discarded here. Carries the result
    /// only so transient failures can be logged.
    RecoveryHeartbeatSent(Result<(), String>),
    /// Terminal no-op for the fire-and-forget re-report of a Cube's Vault
    /// presence after a mid-session Vault creation (the duress vault gate;
    /// PLAN-duress-vault-gate PR 3). The result is logged inside the task and
    /// discarded here — it must never block or affect the UI.
    CubeVaultReported,
    /// An imported Vault's Bitcoin Blake2b copy (its paired Cube) was
    /// created, or why not.
    ImportedCopyCreated(Result<(), String>),
    /// Terminal no-op for the fire-and-forget push of a Spark Stable Balance
    /// decision to the Cube's Connect record
    /// (`ConnectCubePanel::report_spark_stable_balance`). Logged inside the
    /// task; the local record is already durable.
    CubeSparkStableBalanceReported,
    /// The Cube's Connect registration answered with the server's Spark
    /// Stable Balance record (`CubeResponse::spark_stable_balance`). `None`
    /// when no desktop has recorded one (or the API predates the field).
    SparkStableBalanceFromConnect(Option<bool>),
    Fiat(FiatMessage),
    UpdatePanelCache(/* is current panel */ bool),
    View(view::Message),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
    /// Result of an off-UI-thread daemon restart (a backend switch). Carries the
    /// new daemon — or a recovered previous one on failure — so the App can swap
    /// it in without freezing the UI on `daemon.stop()` / `EmbeddedDaemon::start`.
    DaemonRestarted(super::DaemonRestart),
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
    /// An imported wallet's keys stored in this Cube, for the signer picker.
    CubeKeys(Result<Vec<crate::services::imported_keys::LocalKeyRecord>, String>),
    WalletUpdated(Result<Arc<Wallet>, Error>),
    /// A hardware wallet registered an imported Vault's device policy from
    /// Vault Settings. Nothing is written to the wallet's settings.
    DeviceRegistered(Fingerprint, Result<(), Error>),
    Updated(Result<(), Error>),
    /// UI-only signal to recompute collected signatures and re-evaluate the
    /// Sign picker's close/broadcast state after an in-memory PSBT merge —
    /// distinct from `Updated(Ok)`, which additionally marks the spend tx as
    /// persisted (`saved = true`). Emitted speculatively by the Keychain flow
    /// before/independent of the `update_spend_tx` round-trip, so it must not
    /// imply a successful save.
    Reconcile,
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
    /// Show the global "payment received" celebration overlay for an incoming
    /// deposit that just landed in the spendable balance — e.g. a claimed
    /// SideShift-swap deposit. Emitted from `GlobalHome` on auto-claim success;
    /// handled inline in `App::update` (the only place that owns the overlay
    /// state). `context` picks the Kage image (`"spark-receive"`); `amount_sat`
    /// is formatted for display against the user's bitcoin unit; `payment_id`,
    /// when known, is shown with a copy button.
    ShowReceivedCelebration {
        context: String,
        amount_sat: u64,
        payment_id: Option<String>,
    },
    SettingsSaved,
    SettingsSaveFailed(Error),
    /// The session's first post-sync read of the Spark SDK's Stable
    /// Balance state: `(sdk_active, holds_usdb)`. `None` when the bridge
    /// couldn't answer (already logged) — the reconcile is skipped for the
    /// session rather than acting on a guess.
    SparkStableBalanceState(Option<(bool, bool)>),
    /// The reconcile re-applied the Cube's recorded Stable Balance
    /// preference on the bridge. `Ok(enabled)` is the value applied.
    SparkStableBalanceApplied(Result<bool, String>),
    /// Store the Bitcoind handle produced by configure_and_start_internal_bitcoind so
    /// that its LockFile is kept alive for the lifetime of the App.
    SetInternalBitcoind(Bitcoind),
    /// Optional node selected during Connect-first Vault creation has finished
    /// provisioning. Connect remains active until the normal sync promotion.
    BackgroundManagedNodeReady {
        cube: String,
        /// The App-issued token of the attempt that produced this result.
        attempt: u64,
        flavor: String,
        result: Result<(coincubed::config::BitcoindConfig, Bitcoind), String>,
    },
    /// Stage of that background install, for Settings → Node.
    BackgroundManagedNodeProgress {
        cube: String,
        attempt: u64,
        progress: super::cache::BackgroundNodeInstall,
    },
    /// Fired by the bitcoind-sync subscription to trigger a progress probe.
    PollBitcoindSync,
    NodeHistory(super::node_history::Action),
    NodeHistoryCompleted {
        request_epoch: u64,
        app: crate::app::cache::AppGeneration,
        wallet: crate::app::settings::WalletId,
        config: coincubed::config::BitcoindConfig,
        result: Result<Box<super::node_history::Outcome>, String>,
    },
    /// A direct local-node observation, scoped to its App and RPC configuration.
    BitcoindSyncProgress {
        app: crate::app::cache::AppGeneration,
        history_epoch: u64,
        wallet: Option<crate::app::settings::WalletId>,
        config: coincubed::config::BitcoindConfig,
        result: Result<super::LocalNodeSync, String>,
        /// How far back the Vault's history reaches, read only when a switch
        /// to a pruned, synced pending node is being considered; `None`
        /// otherwise or when it could not be read.
        vault_history: Option<super::local_switch::VaultHistory>,
    },
    /// Result of polling the *active* managed node's network stats (connection
    /// counts, upload used vs. cap, onion address) for the Node settings.
    BitcoindNetStats(Result<crate::app::cache::NodeNetStats, String>),
    /// A bounded recovery-notice observation, scoped to App and session.
    UnsweptNotice {
        app: crate::app::cache::AppGeneration,
        generation: u64,
        result: Result<
            Option<crate::services::foreign_scan::known::KnownUnspent>,
            crate::services::foreign_scan::ScanError,
        >,
    },
    /// A sync-driven entangled-deposit lookup batch for a Bitcoin Blake2b
    /// Cube (`#276` I13) has answered: `claimed` is every txid the batch took
    /// in flight (released as a whole), `answers` every answer, `Unknown`
    /// included (only resolved ones are cached; the rest are asked again
    /// after the next sync), each stamped with the instant it was observed.
    /// `origin` names the `App` instance that issued the batch: a batch that
    /// outlived a Cube switch is applied by [`Cache::accept_entanglement`]'s
    /// stale-instance rule, and releases no claim on the instance it lands on
    /// (`#393`).
    ///
    /// [`Cache::accept_entanglement`]: crate::app::cache::Cache::accept_entanglement
    EntangledLookups {
        origin: crate::app::cache::LookupOrigin,
        claimed: Vec<coincube_core::miniscript::bitcoin::Txid>,
        answers: Vec<crate::services::entangled::LookupAnswer>,
    },
    /// Coin control asks the app for what the Split coins overlay needs: the
    /// chain's fee rate and, without a split key, a fresh receive address.
    SplitFetch {
        fee: bool,
        address: bool,
    },
    /// The chain-tagged Connect fee rate for the overlay; `None` is
    /// unavailable, never a guess.
    SplitFeeRate(Option<u64>),
    /// A fresh receive address for a split key, or why there isn't one.
    SplitAddress(Result<String, String>),
    /// Build the split the overlay shows ([`crate::services::split_coins::prepare`]).
    SplitRequest {
        method: crate::services::split_coins::Method,
        coins: Vec<coincube_core::miniscript::bitcoin::OutPoint>,
        key: Option<coincube_core::miniscript::bitcoin::OutPoint>,
        feerate_vb: u64,
    },
    /// The split was built and stored with the daemon, or why not.
    SplitPrepared(Result<Box<crate::services::split_coins::SplitRecord>, String>),
    /// Open Send with exactly these coins chosen.
    SendSelectedCoins(Vec<coincube_core::miniscript::bitcoin::OutPoint>),
    /// The daemon's copies of this Vault's recorded split transactions, for
    /// the Chains column ([`crate::services::split_coins::derive`]).
    SplitTransactions {
        origin: crate::app::cache::LookupOrigin,
        transactions: std::collections::HashMap<
            coincube_core::miniscript::bitcoin::Txid,
            coincube_core::miniscript::bitcoin::Transaction,
        >,
        /// The pass's number; only a newer pass than the last applied one
        /// is applied ([`crate::services::split_coins::newer_pass`]).
        seq: u64,
        /// The twin Cube's split journal as read for this pass.
        twin_records: Vec<crate::services::split_coins::SplitRecord>,
        /// Every twin split this pass nominated, with the inputs of its
        /// transaction if Connect showed it deep enough on the twin chain
        /// ([`crate::services::split_coins::TwinCheck::spent`]).
        twin_checked: Vec<(
            coincube_core::miniscript::bitcoin::Txid,
            Option<std::collections::HashSet<coincube_core::miniscript::bitcoin::OutPoint>>,
        )>,
        /// When the twin lookups started, before any read; orders them
        /// against other passes'.
        twin_checked_at: std::time::Instant,
    },
    /// A sync-driven ancestry check batch for the Chains column finished
    /// ([`crate::services::chain_status::check_ancestry`]). `claimed` is the
    /// batch's single-flight claim; see [`crate::app::cache::Cache::chain_proofs_landed`].
    ChainProofs {
        origin: crate::app::cache::LookupOrigin,
        claimed: Vec<coincube_core::miniscript::bitcoin::OutPoint>,
        results: Vec<(
            coincube_core::miniscript::bitcoin::OutPoint,
            crate::services::chain_status::ProofEntry,
        )>,
    },
    /// The spend screen re-checked the entanglement of a replayable spend's
    /// inputs at the moment it matters (`#276` I13). Every resolved answer is
    /// cached by the app — each under its own observation instant, so a
    /// reply from a screen since closed cannot re-stamp a newer negative —
    /// then the reply is routed to the current panel so the spend clears its
    /// "checking" state; `spend` is the unsigned transaction's txid so a
    /// stale reply for another spend is ignored. `origin` names the `App`
    /// instance whose screen asked (`#393`): a reply from a predecessor
    /// instance is not routed at all.
    EntangledRevalidated {
        origin: crate::app::cache::LookupOrigin,
        spend: coincube_core::miniscript::bitcoin::Txid,
        /// The check generation this reply answers
        /// (`PsbtState::entangled_check`): process-wide monotonic, so a
        /// reply for an earlier generation — an older screen instance, or
        /// an earlier signature set — can never clear the current claim.
        generation: u64,
        answers: Vec<crate::services::entangled::LookupAnswer>,
    },
    /// The spend screen asks for the split key `key` of the spend `spend` to
    /// be re-qualified against both chains
    /// ([`crate::services::split_coins::requalify`]) before it counts toward
    /// signing or broadcast. `generation` is the screen's check generation.
    SplitKeyCheck {
        spend: coincube_core::miniscript::bitcoin::Txid,
        key: coincube_core::miniscript::bitcoin::OutPoint,
        generation: u64,
    },
    /// The answer to [`Self::SplitKeyCheck`], routed to the current panel.
    /// The screen accepts it only for the generation it has in flight.
    SplitKeyChecked {
        spend: coincube_core::miniscript::bitcoin::Txid,
        key: coincube_core::miniscript::bitcoin::OutPoint,
        generation: u64,
        result: Result<(), String>,
    },
    /// Latest UpdateTip/blockheaders line streamed from the pending internal
    /// bitcoind's debug.log.  `None` means no matching line found yet.
    PendingBitcoindLog(Option<String>),
    InstallStats(InstallStatsMessage),
    /// Connect realtime stream event forwarded from the gRPC subscription.
    /// `None` payload means "no grpc_url available; stream stays offline".
    ConnectStreamReady(
        Option<(
            crate::services::connect::grpc::stream::ConnectStreamConfig,
            String,
        )>,
    ),
    /// One message from the live Connect realtime stream subscription.
    /// Routed to the active vault PSBT modal in PR B; PR A logs only.
    ConnectStream(crate::services::connect::grpc::ConnectStreamMessage),
    /// The wait between automatic renewal attempts is over (C1).
    ConnectRenewalRetry,
    /// Flips the pulsing Connect light between its bright and dim phase.
    ConnectLightTick,
    /// Connect, not the user, ended the session: shown as Signed out by
    /// Connect until the user signs in or presses Not now. Sent just before
    /// the account panel's `LogOut`.
    ConnectSignedOutBy(crate::app::SignedOutBy),
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
        epoch: u64,
    },
    /// Commit a verified App-panel OTP or refresh to connect.json before the
    /// panel/keyring can adopt the bearer.
    InAppConnectAuthReady {
        login: crate::services::coincube::LoginResponse,
        epoch: u64,
        /// None for OTP login, Some for a keyring refresh.
        previous_refresh_token: Option<String>,
    },
    InAppConnectAuthPersisted {
        login: crate::services::coincube::LoginResponse,
        epoch: u64,
        previous_refresh_token: Option<String>,
        tokens: Option<crate::services::connect::client::auth::AccessTokenResponse>,
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
    /// On-demand Connect signing bootstrap for an already-authenticated
    /// session. Fired from the "Sign with Connect" action in the
    /// keychain-unavailable modal when the user is signed in but the gRPC
    /// stream/device hasn't been set up (the local-node launch doesn't
    /// register a signer device). Loads tokens from `connect.json`,
    /// registers a device via gRPC, and chains `TriggerConnectStreamReady`
    /// so `cache.connect_*` populate without an app restart.
    EnsureConnectReady,
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
