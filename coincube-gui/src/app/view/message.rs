use crate::{
    app::{
        menu::Menu,
        settings::unit::BitcoinDisplayUnit,
        view::{
            global_home::{
                PickerSide, TransferDirection, TransferError, TransferStage, WalletKind,
            },
            FiatAmountConverter,
        },
    },
    export::ImportExportMessage,
    node::bitcoind::{Bitcoind, NodeFlavor, RpcAuthType},
    services::{
        fiat::{Currency, PriceSource},
        sideshift::{ShiftQuote, ShiftResponse, ShiftStatus, SideshiftNetwork},
    },
    user_error::UserError,
};
use coincubed::config::BitcoindConfig;
use zeroize::Zeroizing;

/// Wrapper around a Connect bearer token that redacts its contents from
/// `Debug` output and zeroes the heap allocation on drop.
///
/// Carried by [`NodeSettingsMessage::SwitchToConnectFastPath`] so the JWT
/// does not leak through `{:?}` on a parent message — `NodeSettingsMessage`
/// derives `Debug`, and tracing/panic dumps elsewhere format messages
/// transitively. Mirrors the pattern used by `CoincubeClient` and
/// `EsploraConfig` (services/coincube/client.rs, coincubed/src/config.rs).
#[derive(Clone)]
pub struct ConnectJwt(Zeroizing<String>);

impl ConnectJwt {
    pub fn new(token: String) -> Self {
        Self(Zeroizing::new(token))
    }

    /// Consume the wrapper and yield the bearer token. The original
    /// `Zeroizing<String>` is dropped here and its heap bytes are wiped;
    /// the returned `String` is a fresh allocation owned by the caller.
    pub fn into_string(self) -> String {
        (*self.0).clone()
    }
}

impl std::fmt::Debug for ConnectJwt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectJwt(<redacted>)")
    }
}

/// Wrapper around the PIN entered to unlock the seed for Full-Cube inheritance
/// escrow. Redacts its contents from `Debug` and zeroes the heap allocation on
/// drop, so the PIN never leaks — neither through `{:?}` on a parent message
/// ([`RecoveryAlertsMessage`] derives `Debug` and tracing/panic dumps format
/// messages transitively) nor through the `RecoveryAlerts` settings state it's
/// stored in. Mirrors [`ConnectJwt`] and the `Zeroizing` secret pattern used
/// across the app.
#[derive(Clone, Default)]
pub struct EscrowPin(Zeroizing<String>);

impl EscrowPin {
    /// Borrow the PIN digits (e.g. to verify against the cube settings).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wipe the entered PIN. Replaces the buffer so the previous bytes are
    /// zeroed on drop — a plain `String::clear` would only reset the length.
    pub fn clear(&mut self) {
        self.0 = Zeroizing::default();
    }
}

impl From<String> for EscrowPin {
    fn from(pin: String) -> Self {
        Self(Zeroizing::new(pin))
    }
}

impl std::fmt::Debug for EscrowPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EscrowPin(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FeeratePriority {
    Low,
    Medium,
    High,
}

use breez_sdk_liquid::prelude::{InputType, PreparePayOnchainResponse, PrepareSendResponse};

use crate::app::wallets::DomainPayment;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Address, OutPoint};
use coincube_core::spend::SpendCreationError;

// Type alias for complex transfer PSBT result
type TransferPsbtResult = Result<
    (
        coincube_core::miniscript::bitcoin::psbt::Psbt,
        Option<std::sync::Arc<crate::app::wallet::Wallet>>,
        (
            crate::dir::CoincubeDirectory,
            coincube_core::miniscript::bitcoin::Network,
        ),
    ),
    String,
>;

pub trait Close {
    fn close() -> Self;
}

/// Coin control intents: choosing coins to send, and the Split coins overlay.
#[derive(Debug, Clone)]
pub enum CoinControlMessage {
    /// Open the label editor for this coin, independently of its details.
    EditLabel(coincube_core::miniscript::bitcoin::OutPoint),
    Choose(coincube_core::miniscript::bitcoin::OutPoint),
    ClearChosen,
    SendSelected,
    OpenSplit,
    CancelSplit,
    ToggleSplitCoin(coincube_core::miniscript::bitcoin::OutPoint),
    SplitMethod(crate::services::split_coins::Method),
    SignSplit,
    /// The send warning's acknowledgement checkbox.
    AcknowledgeBothChains(bool),
    /// Send the selected coins despite the warning.
    SendAnyway,
    CloseWarning,
}

#[derive(Debug, Clone)]
pub enum VaultReceiveMessage {
    Copy(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Activity(crate::app::view::activity::ActivityMessage),
    LocalMcp(crate::app::local_mcp::Message),
    ImportedBitcoin(crate::app::state::imported_bitcoin::ImportMessage),
    /// Open the Branta identity at `(output_index, identity_index)` in the reviewed PSBT.
    OpenVaultRecipientIdentity(usize, usize),
    Scroll(f32),
    Reload,
    Clipboard(String),
    /// Copy text and show confirmation after the clipboard write is requested.
    CopyWithToast(String, String),
    Menu(Menu),
    /// Show the Launcher while keeping this Cube's session in its App tab.
    OpenLauncher,
    /// Open installation-wide preferences above the retained Cube.
    OpenAppPreferences,
    /// Lock through the tab's secure lifecycle, then show the Launcher.
    LockCubeToLauncher,
    /// An import that never finished: lock this Cube and ask to delete it
    /// from the Launcher, through the Launcher's own delete confirmation.
    DeleteUnfinishedCube,
    ToggleCubeSwitcher,
    /// The Connect control in the top bar: opens or closes its panel.
    ToggleConnectPanel,
    /// A press outside the Connect panel, or Escape.
    CloseConnectPanel,
    /// "Not now" on Signed out by Connect: clears the red light and leaves
    /// the control in the ordinary signed-out state.
    DismissConnectSignOut,
    AskCubeLock,
    CancelCubeLock,
    /// A shared Action's pointer-down request. The App applies whole-tree
    /// focus so a captured sidebar click also blurs content controls.
    FocusAction(iced::advanced::widget::Id),
    /// Device-wide privacy intent handled only by the GUI preferences owner.
    ChangeDevicePreference(crate::app::settings::preferences::PreferenceChange),
    SetupVault,
    /// W15 — launch the vault installer in "restore from Connect
    /// Recovery Kit" mode. Branches off the same button the default
    /// `SetupVault` goes through, but flags the installer to use
    /// `UserFlow::RestoreVaultFromRecoveryKit` instead of
    /// `CreateWallet`.
    SetupVaultRestoreFromKit,
    Close,
    Select(usize),
    /// Coin control's checkbox selection and Split coins overlay.
    CoinControl(CoinControlMessage),
    /// Go to Coin control and open the Split coins overlay.
    SplitFirst,
    SelectRefundable(usize),
    RefundAddressEdited(String),
    RefundAddressValidated(bool),
    RefundFeerateEdited(String),
    RefundFeeratePrioritySelected(FeeratePriority),
    RefundFeeratePriorityFailed(String),
    /// Result of the async fee-rate fetch spawned by
    /// `RefundFeeratePrioritySelected`. Carries the originating priority so
    /// the handler can ignore stale responses — e.g. when the user typed a
    /// custom feerate or picked a different priority before this one
    /// returned. `Some(rate)` = success, `None` = fetch failed.
    RefundFeeratePriorityResolved(FeeratePriority, Option<usize>),
    SubmitRefund,
    /// Pull a fresh native-Bitcoin receive address from the Vault wallet and
    /// drop it into the refund address input. This routes through the existing
    /// `daemon.get_new_address()` path so no address-derivation logic is
    /// duplicated here.
    GenerateVaultRefundAddress,
    /// Result of the async `daemon.get_new_address()` call spawned by
    /// `GenerateVaultRefundAddress`. Carries the request id so the handler
    /// can ignore stale responses — e.g. when the user typed their own
    /// address (clearing the pending id) before the Vault lookup returned,
    /// or clicked the button twice. `Ok` = address, `Err` = error message.
    VaultRefundAddressResolved(u64, Result<String, String>),
    /// Dismiss the "pick a date to rescan from" prompt for this session.
    HideRescanPrompt,
    SelectPayment(OutPoint),
    Label(Vec<String>, LabelMessage),
    NextReceiveAddress,
    ToggleShowPreviousAddresses,
    SelectAddress(Address),
    Settings(SettingsMessage),
    CreateSpend(CreateSpendMessage),
    ImportSpend(ImportSpendMessage),
    BuySell(BuySellMessage),
    Spend(SpendTxMessage),
    Next,
    Previous,
    SelectHardwareWallet(usize),
    /// Sign a PSBT with the matching device even if device enumeration changes.
    SelectPsbtHardwareWallet(Fingerprint),
    CreateRbf(CreateRbfMessage),
    ShowQrCode(usize),
    ImportExport(ImportExportMessage),
    ExportPsbt,
    ImportPsbt,
    OpenUrl(String),
    /// Open the installer that creates this Bitcoin Cube's Bitcoin Blake2b
    /// copy (Tools › Split coins). Creates the copy Cube only — nothing is
    /// split, swept or broadcast by this flow.
    StartClaimBlake2b,
    /// Tools › Split coins › Create copy. The App re-checks the disk for an
    /// existing copy before it starts [`Self::StartClaimBlake2b`].
    CreateBlake2bCopy,
    /// Collapse the firmware-advisory detail panel on one device row. Carries
    /// the device fingerprint and the advisory id; the badge itself stays.
    DismissHwAdvisory(Fingerprint, &'static str),
    Home(HomeMessage),
    LiquidOverview(LiquidOverviewMessage),
    LiquidTransactions(LiquidTransactionsMessage),
    SparkOverview(crate::app::view::spark::SparkOverviewMessage),
    SparkTransactions(crate::app::view::spark::SparkTransactionsMessage),
    SparkSettings(crate::app::view::spark::SparkSettingsMessage),
    SparkSend(crate::app::view::spark::SparkSendMessage),
    SparkReceive(crate::app::view::spark::SparkReceiveMessage),
    SparkSwap(crate::app::view::spark::SparkSwapMessage),
    LiquidReceive(LiquidReceiveMessage),
    VaultReceive(VaultReceiveMessage),
    LiquidSend(LiquidSendMessage),
    LiquidSwap(LiquidSwapMessage),
    LiquidSettings(LiquidSettingsMessage),
    PreselectPayment(DomainPayment),
    SetAssetFilter(crate::app::state::liquid::transactions::AssetFilter),
    /// Liquid Transactions: navigate to previous page.
    LiquidPrevPage,
    /// Liquid Transactions: navigate to next page.
    LiquidNextPage,
    /// Vault Transactions: navigate to previous page.
    VaultPrevPage,
    /// Vault Transactions: navigate to next page.
    VaultNextPage,
    ShowError(String),
    ShowSuccess(String),
    ShowToast(log::Level, String),
    /// Keep the incoming amount structured until the toast is rendered, so an
    /// already-visible notice follows the current device privacy choice.
    ShowIncomingLiquidWaitingToast(u64),
    DismissToast(usize),
    SideshiftReceive(SideshiftReceiveMessage),
    SideshiftSend(SideshiftSendMessage),
    /// Spark's "receive from another network" bridge — deposit any supported
    /// asset, receive Bitcoin. Kept separate from [`Self::SideshiftReceive`]
    /// (the Liquid USDt flow) rather than shared: that flow settles Liquid USDt
    /// and is being sunset, this one settles on-chain BTC and collects a refund
    /// address. Folding them together would couple the survivor to the corpse.
    SparkSideshiftReceive(SparkSideshiftReceiveMessage),
    ConnectAccount(ConnectAccountMessage),
    ConnectCube(ConnectCubeMessage),
    P2P(P2PMessage),
    /// Bubbles up from a Connect-requiring feature page (Spark
    /// Settings → Lightning Address, Cube Settings → Avatar / Members)
    /// when an unauthenticated user clicks "Sign In" on the inline
    /// prompt. The Pane intercepts it and focuses the Home tab on its
    /// Connect section.
    OpenConnectSignIn,
    /// Like [`OpenConnectSignIn`], but for a user who is already signed in to
    /// Connect under an account that can't act on this Cube. Always hands off
    /// to the Home tab's Connect overview (where they can log out and sign
    /// back in) instead of short-circuiting because a session already exists.
    OpenConnectSwitchAccount,
    /// Bubbles up from a paid-feature locked card outside the Connect page
    /// (e.g. Settings → Vault Recovery Alerts) when the user clicks "View
    /// plans". The Pane intercepts it and focuses the Home tab on its
    /// Connect → Plan & Billing section. Mirrors [`OpenConnectSignIn`].
    OpenPlanBilling,
    /// A focusable paid-feature CTA records its own return target for the
    /// Connect sheet's Close and Escape route.
    OpenPlanBillingFrom(&'static str),
    /// Remote duress activation arrived over the gRPC stream (Phase 7b). The
    /// tab shell intercepts this and locks the running app into the cryptic
    /// "Duress Mode Activated" screen — WITHOUT wiping (remote activation can
    /// be accidental; only a local duress PIN wipes).
    DuressLockRemote,
    /// A settings PIN trial matched this Cube's local duress marker. The tab
    /// checks the originating App/Cube before invoking the same local wipe
    /// orchestrator used by ordinary PIN unlock; no seed travels in this event.
    DuressActivateLocal(crate::services::unlock::throttle::VerifiedSeedExportDuress),
    ToggleTheme,
    DismissReceivedCelebration,
    /// Answer to the one-time recovery-alerts consent prompt overlay
    /// (PLAN-recovery-alerts-cleanup PR 3). `true` = accept (enable monitoring),
    /// `false` = decline. Handled at the App level; the answer is persisted
    /// per-cube so the prompt never re-fires.
    RecoveryAlertsConsent(bool),
    /// Close the Spark Stable Balance banner for this session
    /// (`Cache::spark_notice`). Handled at the App level.
    DismissSparkNotice,
    /// Flip the global fiat-native ↔ bitcoin-native display preference
    /// and persist it. Emitted by the click-to-swap mouse_area on any
    /// primary balance value, and by the Settings toggle.
    FlipDisplayMode,
}

impl Close for Message {
    fn close() -> Self {
        Self::Close
    }
}

#[derive(Debug, Clone)]
pub enum LabelMessage {
    Edited(String),
    Cancel,
    Confirm,
}

#[derive(Debug, Clone)]
pub enum CreateSpendMessage {
    AddRecipient,
    BatchLabelEdited(String),
    DeleteRecipient(usize),
    SelectCoin(usize),
    RecipientEdited(usize, &'static str, String),
    RecipientFiatAmountEdited(usize, String, FiatAmountConverter),
    FeerateEdited(String),
    SelectPath(usize),
    Generate,
    SendMaxToRecipient(usize),
    FetchFeeEstimate(usize),
    /// A preset's estimate (`None` if the fetch failed), carrying the identity
    /// of the `FetchFeeEstimate` request it answers.
    FeeEstimated {
        request: u64,
        feerate: Option<usize>,
    },
    /// Choose coins: open the coin list instead of the automatic selection.
    ChooseCoins,
    /// The Custom fee segment: reveal the feerate field, restoring the last
    /// rate typed into it.
    CustomFeerate,
    SessionError(SpendCreationError),
    Clear,
}

#[derive(Debug, Clone)]
pub enum ImportSpendMessage {
    Import,
    PsbtEdited(String),
    Confirm,
}

#[derive(Debug, Clone)]
pub enum SpendTxMessage {
    Delete,
    Sign,
    /// Leave the view-only MCP review and show the ordinary save and sign controls.
    #[cfg(feature = "mcp-transactions")]
    ContinueNativeSigning,
    /// Request a Keychain signature from one specific signer, chosen from
    /// the unified signing picker. Carries the descriptor fingerprint of
    /// the keychain signer to address. Creates just that signer's
    /// `SigningSession` (see `KeychainSignModal::request_signer`) rather
    /// than fanning out to everyone at once.
    SelectKeychainSigner(Fingerprint),
    /// Request a Keychain signature from every eligible signer at once —
    /// the explicit "fan out to everyone" affordance. Flips all idle
    /// keychain rows into the requested/waiting state.
    RequestFromEveryone,
    /// Expand/collapse an unavailable spending-path card in the signing picker.
    /// Keyed by the path's recovery timelock: `None` is the primary path (which
    /// is inactive during a recovery spend), `Some(seq)` a recovery path.
    /// Inactive paths default to collapsed (header only).
    ToggleSpendPath(Option<u16>),
    /// Cancel all non-terminal `SigningSession`s tracked by the open
    /// signing picker's nested `KeychainSignModal`. Discards any partial
    /// signatures already returned — those are not merged into the PSBT
    /// until all signers have responded.
    CancelKeychainSign,
    /// Retry a single signer whose session expired or was rejected.
    /// Carries the per-signer position in `KeychainSignModal::pending`.
    RetryKeychainSigner(usize),
    /// From the "Keychain signing needs Connect" modal: dismiss it and
    /// route the user to the Connect sign-in flow.
    KeychainConnectSignIn,
    /// From the modal when the user is already signed in to Connect but the
    /// signing stream isn't ready: dismiss it and kick off the on-demand
    /// Connect bootstrap (device registration + stream) via
    /// `Message::EnsureConnectReady`.
    KeychainEnsureConnect,
    /// From the "Keychain signing needs Connect" modal: dismiss it and
    /// jump to Vault → Settings → Pair so the user can pair a phone over
    /// Wi-Fi and sign locally without Connect.
    KeychainPairPhone,
    /// The user ticked (or cleared) the replay acknowledgement on a
    /// replayable Bitcoin Blake2b spend
    /// (`state::vault::replay::REPLAYABLE_ACKNOWLEDGEMENT`). Broadcast stays
    /// disabled until it is ticked.
    AcknowledgeReplay(bool),
    Broadcast,
    Save,
    Confirm,
    Cancel,
    SelectMasterSigner,
    SelectBorderWallet(Fingerprint),
    BorderWalletRecon(BorderWalletReconMessage),
    EditPsbt,
    PsbtEdited(String),
    Next,
}

/// Messages for the Border Wallet reconstruction wizard within the signing flow.
#[derive(Clone)]
pub enum BorderWalletReconMessage {
    PhraseWordEdited(usize, String),
    Next,
    Previous,
    ToggleCell(u16, u8),
    UndoLastCell,
    ClearPattern,
    Cancel,
}

// Manual Debug impl to redact recovery phrase words from logs.
impl std::fmt::Debug for BorderWalletReconMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PhraseWordEdited(idx, _) => f
                .debug_tuple("PhraseWordEdited")
                .field(idx)
                .field(&"<redacted>")
                .finish(),
            Self::Next => write!(f, "Next"),
            Self::Previous => write!(f, "Previous"),
            Self::ToggleCell(r, c) => f.debug_tuple("ToggleCell").field(r).field(c).finish(),
            Self::UndoLastCell => write!(f, "UndoLastCell"),
            Self::ClearPattern => write!(f, "ClearPattern"),
            Self::Cancel => write!(f, "Cancel"),
        }
    }
}

/// A successful Re-register, tagged with the sign-in it was started from.
/// The App adopts `device_id` into the live stream only while that stream
/// still runs as `email` on `replaces`, so a result landing after an account
/// switch, or after a newer sign-in registered its own id, changes nothing.
#[derive(Debug, Clone)]
pub struct ReregisteredDevice {
    pub email: String,
    pub replaces: String,
    pub device_id: String,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    DataExportsSection,
    EditBitcoindSettings,
    BitcoindSettings(SettingsEditMessage),
    ElectrumSettings(SettingsEditMessage),
    RescanSettings(SettingsEditMessage),
    ImportExport(ImportExportMessage),
    EditWalletSettings,
    ToggleWalletExpert,
    ReviewDescriptorCopy,
    ConfirmDescriptorCopy,
    CancelDescriptorCopy,
    ImportExportSection,
    ExportEncryptedDescriptor,
    ReviewPlaintextDescriptorExport,
    CancelPlaintextDescriptorExport,
    ExportPlaintextDescriptor,
    ExportTransactions,
    ExportLabels,
    ExportWallet,
    ImportWallet,
    AboutSection,
    /// Install Cube › Settings › Connections (Connect account and this
    /// computer's live updates).
    ConnectionsSection,
    /// "Try again" after automatic renewal failed: the App registers this
    /// computer again with its stored key (`renewal_task`).
    ReregisterConnectDevice,
    /// Settled result of a renewal, automatic or Try again: the registration
    /// on success, or an error for the log. Handled by the App.
    ConnectDeviceReregistered(Result<ReregisteredDevice, String>),
    /// Shows or hides Details for support (the device ID) under Couldn't
    /// reconnect on Cube › Settings › Connections.
    ToggleConnectSupportDetails,
    RegisterWallet,
    FingerprintAliasEdited(Fingerprint, String),
    WalletAliasEdited(String),
    Save,
    GeneralSection,
    /// The inline Cube name field on Display & appearance changed.
    CubeNameEdited(String),
    /// Save the Cube name typed on Display & appearance (Return or leaving
    /// the field). The App writes it locally, then to Connect.
    CubeNameSubmitted(String),
    PaymentChecksSection,
    /// Ask before turning recipient checks off (Payment checks).
    AskRecipientChecksOff,
    CancelRecipientChecksOff,
    /// Install the Recovery settings section (backup + Recovery Kit + alerts).
    RecoverySection,
    DisplayUnitChanged(BitcoinDisplayUnit),
    Fiat(FiatMessage),
    NodeSettings(NodeSettingsMessage),
    InstallStatsSection,
    InstallStats(InstallStatsViewMessage),
    ToggleDirectionBadges(bool),
    ToggleRecipientIdentityChecks(bool),
    /// Master seed backup flow (moved from Liquid Settings to Cube/General Settings).
    BackupMasterSeed(BackupWalletMessage),
    BackupMasterSeedUpdated,
    /// Cube Recovery Kit — Connect-hosted encrypted backup of the seed
    /// and/or wallet descriptor. See `PLAN-cube-recovery-kit-desktop.md`.
    /// Coexists with `BackupMasterSeed` above (the local paper-phrase
    /// backup), it does not replace it.
    RecoveryKit(RecoveryKitMessage),
    /// Vault Recovery Alerts — Estate-only chain monitoring opt-in
    /// (descriptor escrow / timelock heartbeat) + keyholder download
    /// policy. See `PLAN-estate-notifications.md` PR 2.
    RecoveryAlerts(RecoveryAlertsMessage),
    /// Local LAN signer ("Paired phones") section — pairs a Keychain
    /// phone over the local network so it shows up in the signer
    /// list. See `PLAN-local-signer-lan-desktop.md`.
    LocalSigningSection,
    LocalSigning(LocalSigningMessage),
}

/// Messages for the Vault Recovery Alerts settings card (Estate
/// Notifications — PR 2). Carries no secrets; the descriptor is built
/// inside the App-level handler from the live wallet, never on the wire of
/// these messages. `Debug` is derived.
#[derive(Debug, Clone)]
pub enum RecoveryAlertsMessage {
    /// Open or close the alerts controls under the Recovery alerts row.
    ToggleManage,
    /// Fetch the vault's monitoring status (resolves the Connect vault id,
    /// then GETs `/vaults/{id}/monitoring`). Fired on entering General
    /// settings and after a successful change.
    LoadStatus,
    /// Async result of `LoadStatus`: `(connectVaultId, recipients, status)`.
    /// The recipients are derived from the same `GET .../vault` response that
    /// resolves the vault id — they're the people the server's recovery sweep
    /// would email, so the card can list exactly that set (see
    /// `services::coincube::recovery_recipients`). The trailing `u64` is the
    /// `session_generation` captured when the load was spawned; the handler
    /// drops the result if the session has since changed (logout / account
    /// switch) so a prior account's vault id + status can't be written into the
    /// reset state.
    StatusLoaded(
        Result<
            (
                u64,
                crate::services::coincube::RecoveryRecipients,
                crate::services::coincube::VaultMonitoringStatus,
            ),
            String,
        >,
        u64,
    ),
    /// Flip the standalone **Recovery alerts** toggle (monitoring on/off),
    /// independent of escrow. `true` posts the monitoring opt-in immediately;
    /// `false` opens the alerts-off confirm dialog (which discloses that it will
    /// also delete any stored keyholder recovery kit) rather than acting silently.
    ToggleAlerts(bool),
    /// Confirm turning alerts off from the dialog. Deletes the monitoring record,
    /// and — when a recovery kit is currently escrowed — deletes the envelope set
    /// too (escrow can't outlive alerts). See the state handler for the ordering.
    ConfirmAlertsOff,
    /// Dismiss the alerts-off confirm dialog without changing anything.
    CancelAlertsOff,
    /// User picked what keyholders can recover (Nothing / Vault-only / Full-Cube).
    /// Nothing deletes the escrow set only (alerts stay on); Vault-only enrols the
    /// descriptor (auto-enabling alerts); Full-Cube first collects the PIN (to
    /// unlock the seed) and enrols on `ConfirmFullCube`.
    SelectEscrow(crate::services::inheritance::EscrowTier),
    /// PIN digit input while enrolling the Full-Cube tier (escrows the seed).
    /// Carried in a redacting, zeroizing [`EscrowPin`] so the PIN never lands
    /// in a `{:?}` dump or lingers un-wiped in a dropped message.
    EscrowPinChanged(EscrowPin),
    /// Confirm Full-Cube enrolment with the entered PIN (unlock seed + escrow).
    ConfirmFullCube,
    /// Abandon the in-progress Full-Cube PIN entry.
    CancelFullCube,
    /// Seed JSON was staged privately after the PIN trial. The handler must
    /// admit this exact attempt before it may begin any escrow network call.
    FullCubePinReady(crate::app::cache::AppGeneration, u64),
    FullCubePinFailed(crate::app::cache::AppGeneration, u64, String),
    FullCubeUploadResult(
        crate::app::cache::AppGeneration,
        u64,
        Result<crate::services::coincube::VaultMonitoringStatus, String>,
    ),
    /// Async result of a level change — the updated status. The `u64` is the
    /// spawn-time `session_generation` (see `StatusLoaded`); a stale result is
    /// dropped rather than clobbering a newer session's state. The trailing
    /// `Option<EscrowTier>` is the tier this operation enrolled / disabled: on
    /// success the handler applies it to the tracked tier. Carrying it in the
    /// result — rather than in shared state — keeps a change that resolves while
    /// another is in flight from applying the wrong operation's tier.
    ChangeResult(
        Result<crate::services::coincube::VaultMonitoringStatus, String>,
        u64,
        Option<crate::services::inheritance::EscrowTier>,
    ),
}

#[derive(Debug, Clone)]
pub enum LocalSigningMessage {
    /// Explicit vault key choice; no default based on descriptor ordering.
    SelectKey(String),
    /// Show or hide each key's full descriptor string under its name.
    ToggleKeyDetails,
    /// Ask Connect which of an older Vault's keys are Keychain keys.
    LookupKeychainKeys,
    /// Answer to [`Self::LookupKeychainKeys`] for the Vault with this
    /// descriptor checksum, already saved to the settings file on success.
    KeychainKeysFound(
        String,
        Result<
            std::collections::HashMap<
                coincube_core::miniscript::bitcoin::bip32::Fingerprint,
                crate::app::state::settings::keychain_backfill::FoundKeychainKey,
            >,
            String,
        >,
    ),
    /// "Pair phone" button → settings state browses mDNS and shows
    /// the phone picker. The user then picks one with `PickPhone`.
    StartPairing,
    /// User picked a phone from the discovered-phones list. Carries
    /// the picked phone's 8-hex cert fingerprint; the settings state
    /// looks it up in its current `PhonePicker.discovered` and uses
    /// the resolved address + service name to build the offer.
    PickPhone(String),
    /// Settings state's pairing-window timer fired one tick. Used to
    /// update the on-screen countdown and (in PhonePicker) to
    /// refresh the mDNS-discovered list.
    Tick,
    /// User cancelled the pairing wizard before the phone connected.
    CancelPairing,
    /// Result of the pairing listener task. The `u64` is the
    /// `pairing_id` that was active when the listener was spawned;
    /// the settings state ignores completions whose id doesn't match
    /// the current run, so a still-in-flight task whose user has
    /// since cancelled or started a new pairing can't stomp on the
    /// UI. `Ok` payload is the persisted [`PairedPhone`]; `Err`
    /// payload is a typed [`PairingError`].
    PairingCompleted(
        u64,
        Result<
            crate::phone_signer::pairing_store::PairedPhone,
            crate::phone_signer::errors::PairingError,
        >,
    ),
    /// User asked to remove a paired phone (8-hex fingerprint of the
    /// phone's cert pin).
    ReviewRemovePhone(String),
    CancelRemovePhone,
    RemovePhone(String),
    /// Periodic tick while no pairing is in progress: tell phones about
    /// removals made here, and ask paired phones whether they removed this
    /// desktop.
    SyncTick,
    /// Result of the pass started by [`Self::SyncTick`].
    PairingsSynced(Result<crate::phone_signer::unpair_sync::SyncReport, String>),
    /// Inline rename draft on a paired-phone row. `(fp8, new_text)`.
    /// Doesn't persist; commit happens on `SaveRow`.
    DraftName(String, String),
    /// Inline fallback-addr (`host:port`) draft on a paired-phone
    /// row. Persists on `SaveRow`. Lets the user dial a phone when
    /// mDNS is blocked.
    DraftFallback(String, String),
    /// Persist the in-memory draft for the given fp8 row.
    SaveRow(String),
}

#[derive(Debug, Clone)]
pub enum InstallStatsViewMessage {
    PeriodChanged(crate::services::coincube::StatsPeriod),
    Refresh,
}

#[derive(Debug, Clone)]
pub enum NodeSettingsMessage {
    RecoveryAdvancedToggled,
    /// Pick where history recovery starts: `true` for a date or block
    /// height, `false` for Connect's history.
    RecoverySourceSelected(bool),
    /// Show or hide the local node's latest log line.
    NodeLogToggled,
    NodeAdvancedToggled,
    RecoveryDismiss,
    RecoveryStartEdited(String),
    RecoveryDateOpen,
    RecoveryDateCancel,
    RecoveryDateSelected(chrono::NaiveDate),
    RecoveryStart,
    ImportConnectHistory,
    RecoveryResume,
    RecoveryPause,
    RecoveryCancel,
    RetentionDaysEdited(String),
    RetentionApply,
    /// Trigger from the "Switch to COINCUBE | Connect" button. Always
    /// rewritten by the App-level dispatcher into either
    /// `SwitchToConnectFastPath(jwt)` (when a Connect session is live) or a
    /// navigation to the Connect tab to sign in. Never reaches the per-panel
    /// state.
    SwitchToConnect,
    /// Carries an existing Connect JWT directly to the per-panel state so the
    /// switch can complete without the user signing in again. The JWT is
    /// wrapped in [`ConnectJwt`] so `{:?}` on this message redacts the token
    /// rather than printing it verbatim.
    SwitchToConnectFastPath(ConnectJwt),
    SwitchToBitcoind,
    /// Switch to the local node even though the Vault is still scanning, after
    /// the user has been told the scan will be discarded.
    ConfirmSwitchDiscardingScan,
    /// Dismiss that confirmation and keep the current backend.
    CancelSwitchDiscardingScan,
    // "Set up local node while on Connect" sub-flow
    SetupLocalNode,
    SetupLocalNodeCancel,
    SetupLocalNodeAddrChanged(String),
    SetupLocalNodeAuthTypeSelected(RpcAuthType),
    SetupLocalNodeFieldEdited(&'static str, String),
    SetupLocalNodeConfirm,
    // Mode picker: false = self-managed external, true = COINCUBE-managed internal
    SetupLocalNodeModeSelected(bool),
    // COINCUBE-managed picker: choose the node flavour (Core or Knots)
    // and begin the managed download/install in one step.
    SetupLocalNodeManagedFlavor(NodeFlavor),
    // Internal (COINCUBE-managed) node setup progress
    SetupLocalNodeDownloadProgress(f32),
    SetupLocalNodeDownloadComplete(Result<Vec<u8>, String>),
    SetupLocalNodeStartResult(Result<(BitcoindConfig, Bitcoind), String>),
    /// Retry the Connect-first managed-node install the App runs in the
    /// background after it failed. Handled by the App, which owns that task.
    RetryBackgroundNodeSetup,
    // "Inbound connections" — inbound-over-Tor preferences. Each persists
    // the preference sidecar; the change takes effect the next time the managed
    // node starts.
    /// Master toggle: make the node reachable over Tor (v3 onion service).
    InboundTorToggled(bool),
    /// Sub-toggle: also route outbound peer connections through Tor.
    InboundTorOutboundToggled(bool),
    /// Cap daily upload at ~1 GB/day (on) vs. unlimited (off).
    InboundTorLimitUploadToggled(bool),
    /// Confirm a pending managed-node flavour switch (download if needed +
    /// restart on the new flavour). Paired with the dropdown's
    /// `SettingsEditMessage::SwitchManagedFlavor`.
    ConfirmFlavorSwitch,
    /// Dismiss the flavour-switch confirmation without switching.
    CancelFlavorSwitch,
    /// Restart the managed node now so freshly-changed inbound-over-Tor settings
    /// take effect (they otherwise apply only on the next node start).
    RestartNodeToApply,
    /// Copy a value (e.g. the onion address) to the clipboard.
    CopyToClipboard(String),
    // "Node resources" editor (prune target + mempool cap) for the internal
    // managed node. Field edits update the pending value; Apply validates and
    // restarts the node so the rewritten bitcoin.conf takes effect.
    /// Edit the custom prune-target field (raw MB string).
    NodeResourcePruneEdited(String),
    /// Edit the custom mempool-cap field (raw MB string; empty = 300 MB default).
    NodeResourceMaxMempoolEdited(String),
    /// One-click "Small computer" preset: 550 MB prune + 100 MB mempool.
    NodeResourceSmallComputer,
    /// One-click "Regular computer" preset: 15 GB prune + default mempool.
    NodeResourceRegularComputer,
    /// Validate the fields and restart the managed node to apply the new prune
    /// target / mempool cap.
    NodeResourceApply,
    // "Your own Esplora" (BYO chain source — `PLAN-connect-blinding` PR D5).
    // By default chain queries go through COINCUBE's proxy (or a public
    // provider); pointing the daemon at the user's own Esplora removes that
    // party from the address-lookup path entirely.
    /// Edit the custom Esplora URL field (raw string, unvalidated).
    CustomEsploraEdited(String),
    /// Probe the entered URL (`GET {url}/blocks/tip/height`) and, if it
    /// answers, switch the daemon's chain backend to it.
    CustomEsploraApply,
    /// Probe result: the tip height the endpoint reported, or why it didn't
    /// answer. A failure never changes the daemon config.
    CustomEsploraProbeResult(Result<u32, String>),
}

#[derive(Debug, Clone)]
pub enum SettingsEditMessage {
    Select,
    FieldEdited(&'static str, String),
    ValidateDomainEdited(bool),
    BitcoindRpcAuthTypeSelected(RpcAuthType),
    Cancel,
    Confirm,
    Clipboard(String),
    /// Managed-node flavour picked from the node-card dropdown. Intercepted by
    /// `BitcoindSettingsState` to raise a confirmation before switching.
    SwitchManagedFlavor(crate::node::bitcoind::NodeFlavor),
}

#[derive(Debug, Clone)]
pub enum CreateRbfMessage {
    New(bool),
    FeerateEdited(String),
    Cancel,
    Confirm,
}

#[derive(Debug, Clone)]
pub enum BuySellMessage {
    // state management
    SessionError(&'static str, String), // inline description + runtime error
    ResetWidget,
    SelectBuyOrSell(super::buysell::BuyOrSell),
    StartSession,
    RefreshLogin {
        refresh_token: String,
    },
    SetLoginState(crate::services::coincube::LoginResponse),
    LogOut,

    // ip geolocation
    CountryDetected(Result<&'static crate::services::coincube::Country, String>),

    // automatic user login
    SubmitLogin,
    LoginSuccess {
        login: crate::services::coincube::LoginResponse,
    },

    // user Registration
    EmailChanged(String),
    SubmitRegistration,
    RegistrationSuccess,

    // OTP Verification
    SendOtp,
    OtpChanged(String),
    OtpCooldownTick,
    VerifyOtp,

    // login to coincube account
    CreateNewAccount,

    // Mavapay specific messages
    Mavapay(crate::services::mavapay::MavapayMessage),

    // Meld specific messages
    Meld(crate::app::view::buysell::meld::MeldMessage),

    // Clipboard action (forwarded to parent Message::Clipboard)
    Clipboard(String),

    ViewHistory,
}

#[derive(Debug, Clone)]
pub enum FiatMessage {
    Enable(bool),
    SourceEdited(PriceSource),
    CurrencyEdited(Currency),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideshiftShiftType {
    Fixed,
    Variable,
}

#[derive(Debug, Clone)]
pub enum SideshiftReceiveMessage {
    SelectNetwork(SideshiftNetwork),
    AmountInput(String),
    Generate,
    AffiliateFetched(Result<String, String>),
    QuoteFetched(Result<ShiftQuote, String>),
    ShiftCreated(Result<ShiftResponse, String>),
    PollStatus,
    StatusUpdated(Result<ShiftStatus, String>),
    Copy,
    /// Go back one step, preserving entered data.
    Back,
    Reset,
    Error(String),
}

/// Messages for the Spark "receive from another network" bridge.
#[derive(Debug, Clone)]
pub enum SparkSideshiftReceiveMessage {
    /// User picked what to deposit, by [`crate::services::sideshift::DepositOption::key`].
    /// Changing chain clears the refund address — it belonged to the old one.
    SelectDeposit(String),
    /// Refund address edited. Validated live against the *origin* chain.
    RefundAddressEdited(String),
    /// Create the shift. Refused unless the refund address validates.
    Generate,
    AffiliateFetched(Result<String, String>),
    ShiftCreated(Result<ShiftResponse, String>),
    PollStatus,
    StatusUpdated(Result<ShiftStatus, String>),
    /// The swap's bitcoin has landed in the Spark wallet and been claimed —
    /// forwarded from the global "bitcoin received" event so the inline flow
    /// can advance from "Bitcoin arriving" to an arrived confirmation. Carries
    /// the actual claimed amount (post network fee), in sats.
    Arrived {
        amount_sat: u64,
    },
    /// Copy the deposit address.
    Copy,
    /// Copy the shift ID (what SideShift support asks for).
    CopyShiftId,
    /// Copy the refund address the user gave.
    CopyRefundAddress,
    /// Flip the in-progress step's pulse phase.
    PulseTick,
    Back,
    Reset,
}

#[derive(Debug, Clone)]
pub enum SideshiftSendMessage {
    /// Address input changed — triggers network auto-detection.
    RecipientAddressEdited(String),
    /// User picks a network when address is ambiguous (0x → ETH/BSC).
    DisambiguateNetwork(SideshiftNetwork),
    /// Proceed from address screen to amount screen.
    Next,
    /// Amount input changed.
    AmountInput(String),
    /// Start the shift creation pipeline.
    Generate,
    AffiliateFetched(Result<String, String>),
    QuoteFetched(Result<ShiftQuote, String>),
    ShiftCreated(Result<ShiftResponse, String>),
    ConfirmSend,
    /// Breez prepare_send_asset succeeded — ready to execute payment.
    PaymentPrepared(breez_sdk_liquid::prelude::PrepareSendResponse),
    /// Breez send_payment completed.
    PaymentSent,
    /// Breez payment failed.
    PaymentFailed(String),
    PollStatus,
    StatusUpdated(String, Result<ShiftStatus, String>),
    /// Go back one step, preserving entered data.
    Back,
    Reset,
    Error(String),
    Copy,
}

/// View-level messages for the Liquid Transactions panel. Currently
/// only carries the SDK-event-driven background refresh signal; the
/// rest of the panel's interactions still go through the generic
/// `view::Message::{Reload, Close, Select, ...}` because most of the
/// state changes were wired before this enum was introduced.
#[derive(Debug, Clone)]
pub enum LiquidTransactionsMessage {
    /// Refresh payments in place without disturbing the user's current
    /// view (selection, refund modal, pagination). The handler gates
    /// the fetch to fire only when the panel is idle on page 0, and
    /// uses `fetch_page(0)` directly rather than the heavy `reload()`
    /// — so a freshly-confirmed Liquid tx surfaces from SDK events
    /// without kicking the user out of any drill-down.
    BackgroundRefresh,
}

#[derive(Debug, Clone)]
pub enum LiquidOverviewMessage {
    SendLbtc,
    ReceiveLbtc,
    SendUsdt,
    ReceiveUsdt,
    /// Navigate to the Liquid Swap screen. Only emitted on mainnet (the
    /// button is hidden off-mainnet, where SideSwap is unavailable).
    Swap,
    History,
    SelectTransaction(crate::app::wallets::types::DomainPaymentIdentity),
    /// Request whole-tree focus after a pointer interaction with a landing action.
    FocusAction(iced::advanced::widget::Id),
    /// Forwarded to the top-level handler to flip the global
    /// fiat-native ↔ bitcoin-native display mode.
    FlipDisplayMode,
    DataLoaded {
        balance: Amount,
        usdt_balance: u64,
        recent_payment: Vec<DomainPayment>,
    },
    Error(String),
    RefreshRequested,
}

#[derive(Debug, Clone)]
pub enum LiquidSendMessage {
    FocusAction(iced::advanced::widget::Id),
    PresetAsset(crate::app::state::liquid::send::SendAsset),
    /// Asset choice within the current Send draft. Selecting the current
    /// segment keeps the typed destination and amount.
    SelectMasterAsset(crate::app::state::liquid::send::SendAsset),
    InputEdited(String),
    /// Carries (original_input, validation_result) so stale async results are discarded.
    InputValidated(String, Option<InputType>),
    Send,
    History,
    SelectTransaction(usize),
    DataLoaded {
        balance: Amount,
        usdt_balance: u64,
        recent_payment: Vec<DomainPayment>,
    },
    Error(String),
    ClearError,
    // Send flow popup messages
    PopupMessage(SendPopupMessage),
    /// A completed regular/asset prepare, carrying the [`PrepareContext`] it was
    /// dispatched under. The context's generation is what lets the state drop a
    /// response for a payment the user has already abandoned, and its snapshot
    /// is what the confirmation screen renders — never the live input fields,
    /// which may by now describe a different payment.
    ///
    /// [`PrepareContext`]: crate::app::state::liquid::send::PrepareContext
    PrepareResponseReceived(
        Box<crate::app::state::liquid::send::PrepareContext>,
        PrepareSendResponse,
    ),
    /// A completed on-chain prepare. Carries the destination address resolved at
    /// dispatch time alongside the context, so execution never re-reads the
    /// address from an input the user may have edited since.
    PrepareOnChainResponseReceived {
        context: Box<crate::app::state::liquid::send::PrepareContext>,
        address: String,
        response: PreparePayOnchainResponse,
    },
    SendMaxPrepared(Result<PrepareSendResponse, String>),
    SendMaxOnChainResult(u64),
    ConfirmSend,
    SendComplete,
    BackToHome,
    LightningLimitsFetched {
        min_sat: u64,
        max_sat: u64,
    },
    OnChainLimitsFetched {
        min_sat: u64,
        max_sat: u64,
    },
    RefreshRequested,
    /// Open the "You Send" asset picker modal.
    OpenSendPicker,
    /// Open the "They Receive" asset+network picker modal.
    OpenReceivePicker,
    /// Close any open picker modal.
    ClosePicker,
    /// Set the "You Send" asset (from the picker).
    SetSendAsset(crate::app::state::liquid::send::SendAsset),
    /// Set the "They Receive" asset + network (from the picker).
    SetReceiveTarget(
        crate::app::state::liquid::send::SendAsset,
        crate::app::state::liquid::send::ReceiveNetwork,
    ),
}

/// Messages for the Liquid cross-asset Swap panel.
///
/// The input is the amount the user wants to *receive* (the `to` asset);
/// the SideSwap quote returns the `from`-asset cost. See
/// [`crate::app::state::liquid::swap`].
#[derive(Debug, Clone)]
pub enum LiquidSwapMessage {
    /// Focus a keyboard action after pointer activation.
    FocusAction(iced::advanced::widget::Id),
    /// Balances + recorded-swap payments loaded for the Swap screen.
    DataLoaded {
        btc_balance: Amount,
        usdt_balance: u64,
        recent_swaps: Vec<DomainPayment>,
    },
    /// Balance/transaction load failed.
    Error(String),
    /// Re-fetch balances on a confirmed SDK `Synced` event (also clears the
    /// sync gate).
    RefreshRequested,
    /// Re-fetch balances only, without asserting sync readiness (e.g. the
    /// post-settlement refresh, whose sync may fail).
    RefreshBalances,
    /// The sync kicked on screen entry finished (`ok` = succeeded). Drives
    /// the sync gate directly so Confirm can't get stuck disabled when no
    /// separate `Synced` event follows (or the sync errored). Tagged with the
    /// reload generation so a stale older sync can't re-gate a newer entry.
    SyncFinished(u64, bool),
    /// A "Last swaps" row was tapped — open it in Transactions.
    SelectSwap(usize),
    /// Self-targeted Liquid receive address generated (or failed) — the
    /// destination every cross-asset prepare/send is pointed at.
    SelfAddressReady(Result<String, String>),
    /// Background rate probe finished — carries the SideSwap rate
    /// (`to`-base per `from`-base) so the rate chip + "Swap All" work
    /// before the user has entered an amount. Failures are ignored. Tagged
    /// with the probe generation so a stale probe (from a prior direction or
    /// session) can't apply its rate.
    RateProbeReady(u64, Result<f64, String>),
    /// "You pay" (from-asset) amount edited — the receive side is derived
    /// from the rate and the quote re-fetched.
    AmountEditedPay(String),
    /// "You receive" (to-asset) amount edited — quoted directly.
    AmountEditedReceive(String),
    /// Swap the `from` / `to` assets and invalidate the stale quote.
    FlipAssets,
    /// Pre-fill the max receivable amount from the current rate, netting
    /// fees, then re-quote (resolves the "cross-asset max needs a quote"
    /// gap noted in send.rs).
    SwapAll,
    /// Debounced trigger to request a quote. Carries the sequence number
    /// it was issued for so stale debounce timers are discarded.
    QuoteRequested(u64),
    /// Quote (or quote error) returned. Tagged with the sequence number
    /// so stale async responses are discarded.
    QuoteReady(u64, Result<PrepareSendResponse, String>),
    /// Advance from the input screen to the locked-quote review.
    Continue,
    /// Return from review to the input screen.
    BackToInput,
    /// Execute the locked quote via `send_payment`.
    Confirm,
    /// Re-fetch the quote (e.g. after expiry on the review screen).
    RefreshQuote,
    /// Per-second countdown tick for the quote-expiry timer.
    QuoteTick,
    /// Swap settled — transition to the success screen. Carries the send
    /// leg's tx id + timestamp so the swap can be recorded locally.
    SwapComplete {
        tx_id: Option<String>,
        timestamp: u32,
    },
    /// `send_payment` failed — surface the error, drop the now-stale
    /// committed quote, and (on review) re-fetch so a retry is fresh.
    SwapFailed(String),
    /// Dismiss the success screen and return to a fresh input.
    Done,
}

#[derive(Debug, Clone)]
pub enum SendPopupMessage {
    AmountEdited(String),
    CommentEdited(String),
    FiatConvert,
    FiatInputEdited(String),
    FiatCurrencySelected(Currency),
    FiatPricesLoaded(std::collections::HashMap<Currency, FiatAmountConverter>),
    FiatDone,
    FiatClose,
    Done,
    Close,
    ToggleSendAsset,
    ToggleFeeAsset,
    SendMax,
    UsdtAmountEdited(String),
}

#[derive(Debug, Clone)]
pub enum LiquidReceiveMessage {
    FocusAction(iced::advanced::widget::Id),
    BackToOverview,
    ToggleMethod(ReceiveMethod),
    Copy,
    ShowQrCode,
    CloseQrCode,
    DismissCelebration,
    GenerateAddress,
    /// Carries the SDK error itself, not its `to_string()`: the receive panel
    /// renders this, and the raw Breez string is a GraphQL/swap-service trace.
    /// `crate::user_error` turns it into copy at the point of display.
    AddressGenerated(
        ReceiveMethod,
        Result<String, crate::app::breez_liquid::BreezError>,
    ),
    AmountInput(String),
    UsdtAmountInput(String),
    DescriptionInput(String),
    Error(String),
    ClearError,
    OnChainLimitsFetched {
        min_sat: u64,
        max_sat: u64,
    },
    LightningLimitsFetched {
        min_sat: u64,
        max_sat: u64,
    },
    /// Open the "You Receive" asset picker modal.
    OpenReceivePicker,
    /// Open the "They Send" network picker modal.
    OpenSenderPicker,
    /// Close any open picker modal.
    ClosePicker,
    /// Set the "You Receive" asset (from the picker).
    SetReceiveAsset(crate::app::state::liquid::send::SendAsset),
    /// Set the "They Send" network (from the picker).
    SetSenderNetwork(SenderNetwork),
    /// Balance and recent transactions loaded from Breez.
    DataLoaded {
        btc_balance: coincube_core::miniscript::bitcoin::Amount,
        usdt_balance: u64,
        recent_payment: Vec<DomainPayment>,
    },
    /// User tapped a recent transaction row.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,
    /// Refresh balance and recent transactions (e.g. after a payment event).
    RefreshRequested,
}

/// Network the sender is sending from (receive flow).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderNetwork {
    /// BTC via Lightning
    Lightning,
    /// L-BTC on Liquid
    Liquid,
    /// BTC on-chain
    Bitcoin,
    /// USDt on Ethereum (SideShift)
    Ethereum,
    /// USDt on Tron (SideShift)
    Tron,
    /// USDt on Binance Smart Chain (SideShift)
    Binance,
    /// USDt on Solana (SideShift)
    Solana,
}

impl SenderNetwork {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lightning => "Lightning",
            Self::Liquid => "Liquid",
            Self::Bitcoin => "Bitcoin",
            Self::Ethereum => "Ethereum",
            Self::Tron => "Tron",
            Self::Binance => "Binance",
            Self::Solana => "Solana",
        }
    }

    pub fn is_sideshift(&self) -> bool {
        matches!(
            self,
            Self::Ethereum | Self::Tron | Self::Binance | Self::Solana
        )
    }

    pub fn to_sideshift_network(&self) -> Option<SideshiftNetwork> {
        match self {
            Self::Ethereum => Some(SideshiftNetwork::Ethereum),
            Self::Tron => Some(SideshiftNetwork::Tron),
            Self::Binance => Some(SideshiftNetwork::Binance),
            Self::Solana => Some(SideshiftNetwork::Solana),
            _ => None,
        }
    }

    /// Valid "They Send" networks for a given "You Receive" asset.
    pub fn options_for_receive_asset(
        asset: crate::app::state::liquid::send::SendAsset,
    ) -> Vec<SenderNetwork> {
        use crate::app::state::liquid::send::SendAsset;
        match asset {
            SendAsset::Lbtc => vec![
                SenderNetwork::Lightning,
                SenderNetwork::Liquid,
                SenderNetwork::Bitcoin,
            ],
            SendAsset::Usdt => vec![
                SenderNetwork::Liquid,
                SenderNetwork::Ethereum,
                SenderNetwork::Tron,
                SenderNetwork::Binance,
                SenderNetwork::Solana,
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveMethod {
    Lightning,
    Liquid,
    OnChain,
    Usdt,
}

#[derive(Debug, Clone)]
pub enum LiquidSettingsMessage {
    ExportPayments,
}

#[derive(Clone)]
pub enum BackupWalletMessage {
    ToggleBackupIntroCheck,
    Start,
    NextStep,
    PreviousStep,
    VerifyPhrase,
    Complete,
    WordInput {
        index: u8,
        input: String,
    },
    /// Digit entry in the backup-flow PIN re-verification gate.
    PinInput(crate::pin_input::Message),
    /// User submits the PIN to unlock the mnemonic.
    VerifyPin,
    /// Async result of PIN verification + mnemonic decryption.
    /// The decrypted seed phrase, wrapped **before** it enters the message
    /// queue. iced clones messages, so a bare `Vec<String>` here meant every
    /// in-flight copy of the seed dropped without being scrubbed — only the
    /// final one stashed in state was `Zeroizing`. Matches
    /// `RecoveryKitMessage::PinVerified`, which already did this.
    PinVerified(Result<zeroize::Zeroizing<Vec<String>>, String>),
    /// PIN-only result fenced to the exact prompt; passkey keeps `PinVerified`.
    PinVerifiedAttempt(
        crate::app::cache::AppGeneration,
        Result<zeroize::Zeroizing<Vec<String>>, String>,
    ),
    /// Async result of persisting `backed_up = true` to settings.json after
    /// the user successfully completed the verification step.
    BackupSaveResult(Result<(), String>),
}

// Manual Debug impl to redact mnemonic words and PIN data from logs.
impl std::fmt::Debug for BackupWalletMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToggleBackupIntroCheck => write!(f, "ToggleBackupIntroCheck"),
            Self::Start => write!(f, "Start"),
            Self::NextStep => write!(f, "NextStep"),
            Self::PreviousStep => write!(f, "PreviousStep"),
            Self::VerifyPhrase => write!(f, "VerifyPhrase"),
            Self::Complete => write!(f, "Complete"),
            Self::WordInput { index, .. } => f
                .debug_struct("WordInput")
                .field("index", index)
                .field("input", &"<redacted>")
                .finish(),
            Self::PinInput(_) => f.debug_tuple("PinInput").field(&"<redacted>").finish(),
            Self::VerifyPin => write!(f, "VerifyPin"),
            Self::PinVerified(Ok(_)) => write!(f, "PinVerified(Ok(<redacted>))"),
            Self::PinVerified(Err(e)) => f
                .debug_tuple("PinVerified")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::PinVerifiedAttempt(_, Ok(_)) => write!(f, "PinVerifiedAttempt(Ok(<redacted>))"),
            Self::PinVerifiedAttempt(_, Err(e)) => f
                .debug_tuple("PinVerifiedAttempt")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::BackupSaveResult(res) => f.debug_tuple("BackupSaveResult").field(res).finish(),
        }
    }
}

/// Which Recovery-Kit mode the user entered the wizard under. Carried
/// inside `RecoveryKitMessage::Start` so the wizard knows whether to
/// prompt for the PIN (mnemonic cubes uploading the seed half) or
/// skip straight to the password screen (passkey cubes, which can
/// only back up the wallet descriptor). Mirrors the plan §2.3 mode
/// matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKitMode {
    /// No server-side kit yet — create the first one. For mnemonic
    /// cubes this uploads the seed blob plus (when a Vault exists)
    /// the descriptor blob. For passkey cubes it's descriptor-only.
    Create,
    /// An existing kit has a seed but no descriptor; add the
    /// descriptor half (and re-encrypt the seed with the new
    /// password, per plan §5).
    AddDescriptor,
    /// An existing kit has a descriptor but no seed; add the seed
    /// half. Not reachable for passkey cubes.
    AddSeed,
    /// Re-encrypt the existing kit under a new password. Keeps both
    /// halves that are present; doesn't add missing halves.
    Rotate,
}

/// How the owner chooses to protect their Recovery Kit, after the seed is
/// unlocked (PLAN-owner-keychain-recovery PR 2). Only shown when
/// `OWNER_KEYCHAIN_RECOVERY_ENABLED`; with the flag off the wizard goes straight
/// to the password path as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryProtectionMode {
    /// The existing Argon2id recovery-password path, unchanged.
    Password,
    /// Seal to the owner's `owner-self` Keychain key (ECIES). No password — the
    /// owner restores by approving a decrypt on their phone.
    Phone,
    /// Both: password blobs **and** phone envelopes; either restores.
    Both,
}

/// Result of the owner-self phone-key detection that runs automatically on
/// entering the protection-choice screen (and via the "Check again" link).
/// Distinguishes the expected "not registered yet" case (`Absent` — show
/// guidance) from a genuine failure (`Failed` — show the error + retry), so the
/// UI can gate the phone options without treating a missing key as an error.
#[derive(Debug, Clone)]
pub enum PhoneKeyOutcome {
    /// A registered `owner-self` recovery key was found — phone options enable.
    Present,
    /// No `owner-self` recipient registered yet (the Keychain app must mint one
    /// first, COIN-390). Phone options stay disabled with guidance copy.
    Absent,
    /// Detection failed (network / auth / not signed in). Display-safe message.
    Failed(String),
}

/// Messages driving the Cube Recovery Kit flow. Mirrors the shape of
/// `BackupWalletMessage` — a Debug impl below redacts every variant
/// that carries key material (mnemonic, password, ciphertext) so the
/// tracing subscriber can still dump messages without leaking.
#[derive(Clone)]
pub enum RecoveryKitMessage {
    /// Fire a `get_recovery_kit_status` request to refresh the cached
    /// status used to render the Settings card. Emitted on page entry
    /// and after any local change that could invalidate the cache
    /// (rotate/remove).
    LoadStatus,
    /// Async result of `LoadStatus`. `Ok(None)` means the backend
    /// returned 404 (no kit on this cube yet).
    StatusLoaded(Result<Option<crate::services::coincube::RecoveryKitStatus>, String>),
    /// User clicked the card CTA — enter the wizard in the given
    /// mode. Kicks off PIN entry for mnemonic cubes or jumps
    /// straight to password entry for passkey cubes.
    Start(RecoveryKitMode),
    /// User hit "Cancel" or back-arrow inside the wizard — drop
    /// transient state (PIN, password, decrypted mnemonic) and
    /// return to the card view.
    Cancel,
    /// Digit entry in the PIN re-verification gate (mnemonic cubes
    /// only — the PIN unlocks the on-disk encrypted mnemonic so the
    /// seed blob can be built).
    PinInput(crate::pin_input::Message),
    /// User submitted the PIN.
    VerifyPin,
    /// Async result: `Ok(words)` on correct PIN + successful
    /// decryption; `Err(msg)` on wrong PIN or disk error. The
    /// mnemonic is wrapped in `Zeroizing` so every in-flight copy
    /// of this message (Iced's runtime may clone between the
    /// update handler, task, and view) is wiped on drop. Without
    /// the wrap, a plain `Vec<String>` with the phrase bytes would
    /// linger on the heap past the message cycle.
    PinVerified(Result<zeroize::Zeroizing<Vec<String>>, String>),
    /// PIN-only result fenced to the exact prompt; passkey keeps `PinVerified`.
    PinVerifiedAttempt(
        crate::app::cache::AppGeneration,
        Result<zeroize::Zeroizing<Vec<String>>, String>,
    ),
    /// Recovery password input changed. Wrapped in `Zeroizing` at
    /// the message level (not just on the state field) because
    /// Iced's runtime clones messages between update/task/view —
    /// plain `String` copies would linger on the heap past the
    /// flow's completion. Matches the installer's
    /// `RecoveryKitRestoreMsg::PasswordEdited` discipline.
    PasswordChanged(zeroize::Zeroizing<String>),
    /// "Confirm recovery password" input changed. Same
    /// `Zeroizing`-at-message-level discipline as
    /// `PasswordChanged` above.
    ConfirmChanged(zeroize::Zeroizing<String>),
    /// User picked a protection mode on the choice screen (PR 2). Routes to the
    /// password path (`Password`/`Both`) or runs the phone seal (`Phone`).
    SelectProtectionMode(RecoveryProtectionMode),
    /// Re-run the owner-self phone-key detection on the choice screen. Fired
    /// automatically on entering the screen, and by the "Check again" link.
    /// Provisioning is phone-initiated (COIN-390): the Keychain app mints +
    /// registers the `owner-self` recovery recipient. This action only *detects*
    /// it — polls the recipients list to see whether the row exists yet.
    ProvisionPhone,
    /// Async result of [`Self::ProvisionPhone`] (the detect). Carries the
    /// three-way [`PhoneKeyOutcome`] so the choice screen can enable the phone
    /// options (`Present`), show guidance (`Absent`), or surface an error.
    ProvisionResult(PhoneKeyOutcome),
    /// Async result of the phone seal+upload (PR 2). On success carries the
    /// SHA-256 fingerprint of the descriptor that was sealed, so the handler can
    /// persist the keychain drift slot (per-method drift, PR 3); the hex is
    /// redacted from `Debug` (a stable cross-session identifier). The error
    /// string is display-safe.
    PhoneSealResult(Result<Option<String>, String>),
    /// User toggled the "I've written this down" gate on the password
    /// screen. Submit is inert until this is true.
    AcknowledgeToggled(bool),
    /// User clicked Submit on the password screen. Triggers the
    /// build-blob → encrypt → upload async task.
    SubmitPassword,
    /// Async result of the encrypt+upload round-trip. The payload is
    /// the `(updated_at, descriptor_fingerprint_hex)` tuple the
    /// settings state needs to cache.
    UploadResult(Result<RecoveryKitUploadOutcome, String>),
    /// User dismissed the Completed screen — return to card view and
    /// trigger a fresh `LoadStatus`.
    DismissCompleted,
    /// User clicked Remove on the card. Enters the `ConfirmRemove` confirmation
    /// screen — it does **not** delete anything (master F5: removal is
    /// destructive and irreversible, so it's gated behind an explicit confirm).
    Remove,
    /// User confirmed removal on the `ConfirmRemove` screen. Fires the actual
    /// delete set — `delete_recovery_kit` for the password half and/or
    /// `delete_recovery_kit_recipient` (cascades envelopes) for the phone half,
    /// run sequentially; partial failure fails loudly.
    ConfirmRemove,
    /// Async result of the removal. The error carries [`RemoveFailure`] so the
    /// handler can tell whether the password kit was already torn down before
    /// the failure and drop the now-stale local fingerprint slot accordingly.
    RemoveResult(Result<(), RemoveFailure>),
}

/// Failure payload from the removal flow ([`remove_backups`]). The password
/// Recovery Kit delete runs **first and unconditionally**, so any failure
/// *after* it (the Keychain teardown or the phone-copy verification) leaves the
/// local password-kit state (`recovery_kit_password_backed_up` and its
/// descriptor fingerprint) describing a kit Connect no longer holds.
/// `password_kit_deleted` lets the error handler clear just that method so
/// `has_recovery_kit()` / `connect_state()` don't keep reporting a password
/// backup that's gone. No sensitive fields — `message` is
/// the user-facing error copy — so `Debug` is derived.
#[derive(Clone, Debug)]
pub struct RemoveFailure {
    /// User-facing error copy, shown via `ShowError` and stored on the
    /// `Error` flow state.
    pub message: String,
    /// Whether the password kit was already deleted server-side before the
    /// failure. `false` only when the password delete itself was the failing
    /// step (the kit is still present, so the local slot is still accurate).
    pub password_kit_deleted: bool,
}

/// What the upload handler hands back to the state machine on success.
/// We only carry the fields the state needs to render the Completed
/// screen and persist the drift-fingerprint cache.
///
/// `Debug` is manual below — the `descriptor_fingerprint` hex is a
/// stable identifier correlatable across sessions and against the
/// server-side record, so it's redacted from any `{:?}` site.
/// Tracing dumps see `Some(<redacted>)` / `None` while still
/// revealing whether an upload produced a fingerprint at all (a
/// non-sensitive signal useful for diagnostics).
#[derive(Clone)]
pub struct RecoveryKitUploadOutcome {
    /// RFC 3339 timestamp from the server's `updatedAt` field. Shown
    /// in the Completed screen's "Last updated" line.
    pub updated_at: String,
    /// `has_encrypted_seed` per the server's response (so the card
    /// can pick the right next-state copy without a second round-trip).
    pub now_has_seed: bool,
    /// `has_encrypted_wallet_descriptor` per the server's response.
    pub now_has_descriptor: bool,
    /// SHA-256 (hex) over the `DescriptorBlob` plaintext we just
    /// uploaded. `None` when the upload didn't include a descriptor
    /// blob. Settings state persists this into
    /// `CubeSettings::recovery_kit_last_backed_up_descriptor_fingerprint`
    /// for drift detection (W12).
    pub descriptor_fingerprint: Option<String>,
}

impl std::fmt::Debug for RecoveryKitUploadOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryKitUploadOutcome")
            .field("updated_at", &self.updated_at)
            .field("now_has_seed", &self.now_has_seed)
            .field("now_has_descriptor", &self.now_has_descriptor)
            .field(
                "descriptor_fingerprint",
                // Preserve presence/absence for diagnostics while
                // hiding the hex itself.
                &self.descriptor_fingerprint.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

// Manual Debug: every variant that could carry mnemonic, password,
// or ciphertext bytes is redacted. Matches `BackupWalletMessage`'s
// pattern above — losing a tracing snapshot must not leak the kit.
impl std::fmt::Debug for RecoveryKitMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadStatus => write!(f, "LoadStatus"),
            Self::StatusLoaded(r) => f.debug_tuple("StatusLoaded").field(r).finish(),
            Self::Start(m) => f.debug_tuple("Start").field(m).finish(),
            Self::Cancel => write!(f, "Cancel"),
            Self::PinInput(_) => f.debug_tuple("PinInput").field(&"<redacted>").finish(),
            Self::VerifyPin => write!(f, "VerifyPin"),
            Self::PinVerified(Ok(_)) => write!(f, "PinVerified(Ok(<redacted>))"),
            Self::PinVerified(Err(e)) => f
                .debug_tuple("PinVerified")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::PinVerifiedAttempt(_, Ok(_)) => write!(f, "PinVerifiedAttempt(Ok(<redacted>))"),
            Self::PinVerifiedAttempt(_, Err(e)) => f
                .debug_tuple("PinVerifiedAttempt")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::PasswordChanged(_) => f
                .debug_tuple("PasswordChanged")
                .field(&"<redacted>")
                .finish(),
            Self::ConfirmChanged(_) => f
                .debug_tuple("ConfirmChanged")
                .field(&"<redacted>")
                .finish(),
            Self::SelectProtectionMode(m) => {
                f.debug_tuple("SelectProtectionMode").field(m).finish()
            }
            Self::ProvisionPhone => write!(f, "ProvisionPhone"),
            Self::ProvisionResult(r) => f.debug_tuple("ProvisionResult").field(r).finish(),
            // Redact the descriptor fingerprint hex (a stable cross-session
            // identifier), preserving Ok/Err and presence for diagnostics.
            Self::PhoneSealResult(Ok(fp)) => f
                .debug_tuple("PhoneSealResult")
                .field(&Ok::<_, ()>(fp.as_ref().map(|_| "<redacted>")))
                .finish(),
            Self::PhoneSealResult(Err(e)) => f
                .debug_tuple("PhoneSealResult")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::AcknowledgeToggled(b) => f.debug_tuple("AcknowledgeToggled").field(b).finish(),
            Self::SubmitPassword => write!(f, "SubmitPassword"),
            Self::UploadResult(Ok(o)) => f.debug_tuple("UploadResult(Ok)").field(o).finish(),
            Self::UploadResult(Err(e)) => f
                .debug_tuple("UploadResult")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::DismissCompleted => write!(f, "DismissCompleted"),
            Self::Remove => write!(f, "Remove"),
            Self::ConfirmRemove => write!(f, "ConfirmRemove"),
            Self::RemoveResult(r) => f.debug_tuple("RemoveResult").field(r).finish(),
        }
    }
}

impl From<FiatMessage> for Message {
    fn from(value: FiatMessage) -> Self {
        Message::Settings(SettingsMessage::Fiat(value))
    }
}

impl From<SettingsMessage> for Message {
    fn from(value: SettingsMessage) -> Self {
        Message::Settings(value)
    }
}

/// What the error card's "Try again" button re-runs.
///
/// The card used to dispatch [`ConnectAccountMessage::Init`] no matter which
/// operation had failed, which made the button wrong in two directions: on the
/// dashboard `Init` returns early and the button did nothing, and on the
/// code-entry screen it fell through to the login form and threw the user off
/// the screen they were on.
///
/// Only operations that can be re-run from the card appear here. A failure on a
/// screen that already has its own control — the sign-in form's Continue, the
/// code screen's Verify, a dialog's Submit — carries no action: that control is
/// the retry, and a second one beside it is noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAction {
    /// Re-read the stored session and re-run the token refresh. This is the
    /// reported bug's path: a refresh that timed out.
    Session,
    /// Reload the Security tab's verified devices and login activity.
    SecurityData,
    /// Reload the signed-in user's profile.
    UserProfile,
    /// Reload the billing history list.
    BillingHistory,
}

/// Account-level Connect messages (login/session, plan, security, etc.).
#[derive(Debug, Clone)]
pub enum ConnectAccountMessage {
    AdmittedUserLoaded {
        user: Result<crate::services::coincube::User, (crate::user_error::UserError, bool)>,
        generation: u64,
        /// The panel's authentication epoch when the load was spawned
        /// (`ConnectAccountPanel::auth_epoch`).
        epoch: u64,
    },
    Init,
    RefreshSession {
        refresh_token: String,
    },
    /// A new OTP login result. The number is the panel's authentication
    /// epoch when the operation that produced it was spawned: a completion
    /// of an operation begun before a global auth invalidation carries an
    /// older epoch than one begun after, whatever order they arrive in.
    SetSession(crate::services::coincube::LoginResponse, u64),
    OtpVerified(crate::services::coincube::LoginResponse, u64, u64),
    OtpVerifyFailed(crate::user_error::UserError, u64, u64),
    /// A keyring session refresh updates the stored bearer without starting
    /// the new-login device registration and stream bootstrap.
    RefreshedSession(crate::services::coincube::LoginResponse, u64, String),
    /// Result persisted to connect.json before the App panel adopts it.
    AuthCommitAccepted {
        login: crate::services::coincube::LoginResponse,
        epoch: u64,
        refreshed: bool,
    },
    AuthCommitRejected {
        epoch: u64,
    },
    SessionLoaded {
        user: crate::services::coincube::User,
        plan: Option<crate::services::coincube::ConnectPlan>,
        /// Inherited from the `SetSession` (or admitted-user load) that
        /// produced this, never the epoch current when it is processed.
        epoch: u64,
    },
    PlanLoaded(Option<crate::services::coincube::ConnectPlan>, u64),
    /// A billing plan reload: session generation, then reload sequence
    /// (only the newest applies).
    PlanReloaded(Option<crate::services::coincube::ConnectPlan>, u64, u64),
    /// Lightweight Account Overview counts (contacts, cubes), fetched on
    /// entering the Overview tab. Each is `None` when its fetch failed —
    /// the row simply hides rather than surfacing an error. Carries
    /// `session_generation` for stale-response guarding.
    OverviewCountsLoaded {
        contacts: Option<usize>,
        cubes: Option<usize>,
        /// Same `list_cubes` response, retained for the sanitized Recovery
        /// services projection without another request.
        recovery: Result<Vec<crate::services::coincube::CubeResponse>, UserError>,
        generation: u64,
        request_seq: u64,
    },
    /// Refresh public Cube metadata for Account → Recovery services.
    RefreshRecoveryServices,
    /// Ask the Launcher to resolve this server Cube to an exact local Cube
    /// before opening its ordinary authenticated unlock path.
    OpenRecoveryService(crate::app::state::connect::account::RecoveryServiceTarget),
    /// A session refresh failed for a reason that is *not* an auth rejection
    /// (a timeout, an offline device, a 5xx). Carries presentation-ready copy
    /// rather than a raw error string — see [`crate::user_error`].
    RefreshFailed(UserError),
    /// A refresh's authentication failure (401/403), with the epoch of the
    /// refresh operation. A current one logs out — re-dispatched as `LogOut`
    /// so the GUI broadcasts it — an obsolete one, begun before an
    /// invalidation and completed after a newer sign-in, is dropped.
    RefreshRejected {
        epoch: u64,
    },
    /// A refresh that never reached Connect: no network, or Connect didn't
    /// answer. The stored session is kept and re-checked (`RecheckSession`)
    /// instead of falling back to the sign-in form.
    RefreshUnreachable {
        trouble: crate::services::connect::grpc::trouble::Trouble,
        epoch: u64,
    },
    /// The wait before re-checking an unverified session is over.
    RecheckSession,
    LogOut,
    /// Open or dismiss the account's inline sign-out review.
    ReviewSignOut(bool),
    EmailChanged(String),
    SubmitLogin,
    SubmitRegistration,
    CreateAccount,
    OtpRequested {
        email: String,
        is_signup: bool,
    },
    OtpChanged(String),
    /// Return from token entry to the same email form, retaining signup mode.
    BackToEmail,
    /// A cooldown tick belongs only to the OTP attempt that scheduled it.
    OtpCooldownTick(u64),
    ResendOtp,
    /// A resend completion belongs only to the OTP attempt that requested it.
    OtpResent(u64),
    OtpResendFailed(UserError, u64),
    VerifyOtp,
    EmailNotVerified {
        email: String,
    },
    VerifiedDevicesLoaded(Vec<crate::services::coincube::VerifiedDevice>, u64),
    LoginActivityLoaded(Vec<crate::services::coincube::LoginActivity>, u64),
    DeleteVerifiedDevice(u32),
    /// Open or dismiss the inline removal review for one verified device.
    ReviewVerifiedDeviceRemoval(Option<u32>),
    VerifiedDeviceDeleted(u32, u64, Result<(), String>),
    CopyToClipboard(String),
    Contacts(ContactsMessage),
    /// A Connect-account operation failed. Carries presentation-ready copy;
    /// never a raw `CoincubeError` string.
    ///
    /// Every sender of this variant is an action started from a form or dialog
    /// that is still on screen, so the resulting card offers no button of its
    /// own — see [`RetryAction`].
    Error(UserError),
    /// The Security tab's background loads failed. Separate from [`Self::Error`]
    /// because this one *is* re-runnable from the card.
    ///
    /// Carries the session generation for the same reason its success siblings
    /// do: a request that fails after a logout or a re-login must not raise a
    /// retryable card on the session that replaced it. See
    /// `ConnectAccountPanel::clear_session`.
    SecurityDataFailed(UserError, u64),
    /// The error card's "Try again" was pressed.
    Retry(RetryAction),
    // --- Plan & Billing ---
    /// Focus a Tenshu control (segmented choice, plan row, Copy button)
    /// after pointer activation, as `Action` requires.
    FocusAction(iced::advanced::widget::Id),
    FeaturesLoaded(Option<crate::services::coincube::FeaturesResponse>, u64),
    BillingCycleSelected(crate::services::coincube::BillingCycle),
    /// Change plan: a plan row was picked (not yet confirmed).
    SelectPlanTier(crate::services::coincube::PlanTier),
    /// Change plan → Review: open the Review step for this tier at the
    /// selected billing cycle. Nothing is charged until "Create invoice".
    ReviewPlanChange(crate::services::coincube::PlanTier),
    StartCheckout(crate::services::coincube::PlanTier),
    /// Create result: session generation, then checkout generation.
    /// The error carries the message and whether Connect refused it (4xx).
    CheckoutCreated(
        Result<crate::services::coincube::CheckoutResponse, (String, bool)>,
        u64,
        u64,
    ),
    /// Poll the charge of checkout generation `u64`.
    PollChargeStatus(u64),
    /// Poll result: session generation, then checkout generation.
    ChargeStatusUpdated(
        Result<crate::services::coincube::ChargeStatusResponse, (String, bool)>,
        u64,
        u64,
    ),
    /// Review: pick the billing period for this purchase.
    SetReviewCycle(crate::services::coincube::BillingCycle),
    /// Awaiting payment: show the Lightning invoice or the on-chain address.
    SelectPaymentNetwork(crate::app::state::connect::account::PaymentNetwork),
    /// Paying in browser → back to the QR code here.
    ShowInvoiceHere,
    /// Polling stopped after errors: check the same invoice again.
    ResumeChargePolling,
    DismissCheckout,
    OpenCheckoutUrl(String),
    /// User dismissed the pre-expiry renewal banner for this session (D1).
    DismissRenewalBanner,
    /// Renewal banner CTA — open Plan & Billing checkout pre-selected to
    /// the current tier + cycle (D1).
    RenewCurrentPlan,
    /// Navigate to account-first Plan & Billing management.
    OpenPlanBilling,
    /// Show or close the server-priced plan comparison within Plan & Billing.
    ComparePlans(bool),
    /// Show or close the optional referral-code field.
    ShowCampaignRedeem(bool),
    /// Billing history load result, with the session generation it belongs to.
    ///
    /// The error is a [`UserError`], converted from the real `CoincubeError` at
    /// the async boundary, for the same reason [`Self::UserProfileFailed`] is:
    /// stringifying it first throws away the status, so the handler cannot tell
    /// a timeout from a plan gate and has to guess at whether retrying could
    /// possibly help.
    BillingHistoryLoaded(
        Result<Vec<crate::services::coincube::BillingHistoryEntry>, UserError>,
        u64,
    ),
    ToggleBillingHistory,
    /// User profile refreshed (billing history view update). The `u64` is the
    /// session generation — see [`Self::SecurityDataFailed`].
    UserProfileLoaded(crate::services::coincube::User, u64),
    /// User profile refresh failed (non-auth error), with the session
    /// generation it belongs to.
    UserProfileFailed(UserError, u64),
    // --- Campaign code redemption (v2 campaign engine) ---
    /// Settings → Plan code field edited.
    CampaignCodeChanged(String),
    /// Submit the settings → Plan code to `POST /connect/campaigns/redeem`.
    RedeemCampaignCode,
    /// Redeem result: `Ok(server message)` / `Err(server message)`. The
    /// `u64` is the session generation guarding stale completions.
    CampaignRedeemed(Result<String, String>, u64),
    /// Optional code field on the account-creation screen edited. Stashed at
    /// registration and redeemed once the new session authenticates.
    RegisterCampaignCodeChanged(String),
    // --- Duress (Phases 6 & 8) ---
    /// Result of the post-sign-in `get_duress_state` gate, classified so the
    /// handler can tell a transient failure (retry) from a contract mismatch
    /// (stop retrying — futile) from a rejected session (re-auth). `Ok(active)`
    /// switches to the recovery flow; `Ok(!active)` reveals the dashboard. The
    /// `u64` is the session generation and the `u8` the attempt count.
    DuressStateChecked(crate::services::coincube::DuressCheckOutcome, u64, u8),
    /// User tapped Retry on the post-login duress verification gate after the
    /// check failed (retries exhausted).
    RetryDuressCheck,
    /// Completion of the background "register this device's duress code" task
    /// (Phase 0). No-op at the UI level — the work is a side effect.
    DuressDeviceRegistered,
    /// Recovery flow + enrollment wizard messages (nested to keep this enum
    /// tidy).
    Duress(DuressMessage),
    /// Duress "Emergency contacts" management (Estate Notifications — PR 1).
    DuressContacts(DuressContactsMessage),
}

/// Messages for the duress "Emergency contacts" section (Estate
/// Notifications — PR 1), nested under
/// [`ConnectAccountMessage::DuressContacts`]. Carries account-scoped PII
/// (names, phones, emails) but no secrets, so `Debug` is derived — the
/// same posture as `ContactsMessage`, which already prints contact emails.
#[derive(Debug, Clone)]
pub enum DuressContactsMessage {
    /// Async result of the contacts load. Carries `session_generation` for
    /// stale-response guarding.
    Loaded(
        Result<Vec<crate::services::coincube::DuressAlertContact>, String>,
        u64,
    ),
    /// Open the add form (empty fields).
    ShowAddForm,
    /// Open the edit form for an existing contact id.
    EditContact(u64),
    /// Leave the form, return to the list.
    BackToList,
    NameChanged(String),
    PhoneChanged(String),
    EmailChanged(String),
    ToggleSms(bool),
    ToggleWhatsapp(bool),
    ToggleEmailChannel(bool),
    /// Submit the add/edit form (POST or PATCH depending on `editing_id`).
    Submit,
    /// Async result of a create/update. Carries `session_generation`.
    SubmitResult(Result<(), String>, u64),
    /// Delete a contact by id.
    Delete(u64),
    /// Async result of a delete. `(contact_id, result, session_generation)`.
    DeleteResult(u64, Result<(), String>, u64),
}

/// Messages for the duress recovery flow (Phase 6) and enrollment wizard
/// (Phases 2 & 8), nested under [`ConnectAccountMessage::Duress`].
///
/// `Debug` is hand-written (not derived) because the `*Changed` variants carry
/// secret keystrokes — duress/regular PINs, the all-clear and CRK passwords —
/// that would otherwise leak into any tracing snapshot of a parent message
/// (this enum bubbles up through `ConnectAccountMessage` → `view::Message` →
/// top-level `Message`, all of which derive `Debug`).
#[derive(Clone)]
pub enum DuressMessage {
    // ── Recovery (Phase 6) ──
    RecoveryPassphraseChanged(String),
    SubmitClear,
    ClearResult(Result<(), String>, u64),
    ForgotAllClear,
    /// After a successful clear, leave recovery and enter the normal dashboard
    /// (where restore-from-CRK is reachable).
    FinishRecovery,

    // ── Enrollment wizard (Phases 2 & 8) ──
    /// Begin enrollment. Duress is a paid (Pro/Estate) feature behind a hard,
    /// server-verified Recovery-Kit gate — a single enrollment path, no tiers.
    StartEnrollment,
    CancelEnrollment,
    EnrollBack,
    EnrollNext,
    DuressPinChanged(String),
    DuressPinConfirmChanged(String),
    AllClearChanged(String),
    CrkPasswordChanged(String),
    DelaySelected(crate::services::duress::enroll::DuressDelay),
    MemorizedToggled(bool),
    SubmitEnrollment,
    /// Result of the local duress-PIN collision pre-flight, which now runs off
    /// the UI thread: checking a PIN means trial-decrypting each Cube's seed
    /// file (~831 ms per Cube), not comparing a 27 ms hash. `Ok(())` resumes
    /// the server enrollment; `Err` shows the message and stops.
    EnrollPreflightDone(Result<(), String>, u64),
    EnrollResult(Result<(), String>, u64),
    /// Mainnet cubes + per-cube recovery-kit status, loaded for the duress
    /// intro screen's "set up a Recovery Kit for each Cube" checklist.
    /// `None` when the fetch failed, so a transient error doesn't blank a
    /// previously-loaded list. Carries `session_generation` (session guard)
    /// and a per-fetch load token (`duress_cubes_seq`, in-flight ordering
    /// guard) so only the newest fetch is applied.
    CubesLoaded(
        Option<Vec<crate::app::state::connect::DuressCube>>,
        u64,
        u64,
    ),
    /// Re-fetch the checklist cube list + recovery-kit status. Fired by the
    /// "Couldn't verify — retry" affordance on an `Unknown` checklist row
    /// (PLAN-duress-vault-gate PR 2), so a transient status-probe failure can
    /// be retried without leaving the tab.
    ReloadCubes,
    /// Server-side duress state (enrolled / active), loaded on entering the
    /// Duress tab so the screen can show the enabled state instead of the
    /// setup flow. `None` when the fetch failed. Carries `session_generation`.
    StateLoaded(Option<crate::services::coincube::DuressState>, u64),
    /// Local persistence of the enrollment succeeded (the duress PIN was armed
    /// on every Cube and `DuressLocalState` written). Emitted by the persist
    /// step's success path so the panel reflects the enabled state only once
    /// it's actually true — never on the optimistic pre-persist path, where a
    /// later collision / disk failure would leave a false "enabled" view.
    EnrollmentPersisted,

    // ── Disable (Issue 2 — turn duress off) ──
    /// User tapped "Disable Duress Mode" in Settings → opens the step-up
    /// re-auth dialog. Which factor it asks for is not known yet; the dialog
    /// opens in a probing state and [`Self::DisableMethodProbed`] settles it.
    DisableStart,
    /// Which step-up factor this device can offer, from
    /// `duress_step_up_method_blocking`. Decides whether the dialog shows a PIN
    /// field, a passkey button, or a refusal.
    DisableMethodProbed(Result<crate::app::DuressStepUpMethod, String>, u64),
    /// The step-up regular-PIN input changed.
    DisablePinChanged(String),
    /// Dismiss the step-up dialog without disabling.
    DisableCancel,
    /// Confirm the step-up: verify the regular Cube PIN locally, then call
    /// `disable_duress()` on the server.
    DisableSubmit,
    /// Confirm the step-up on a passkey-only device: raise the system passkey
    /// prompt. Converges on [`Self::DisableStepUpDone`] like the PIN path, so
    /// both factors share one route to the server call.
    DisablePasskeySubmit,
    /// Result of the local step-up PIN check, which now runs off the UI thread
    /// for the same reason as [`Self::EnrollPreflightDone`]. `Ok(())` proceeds
    /// to the server `disable_duress()` call.
    DisableStepUpDone(Result<(), String>, u64),
    /// Result of the server `disable_duress()` call. `Active` (423) means duress
    /// is currently active and can't be disabled.
    DisableResult(Result<(), DuressDisableError>, u64),
    /// Result of the local disarm (`clear_duress_enrollment` /
    /// `reconcile_duress_disarm`). `Ok(true)` = a disarm actually ran (drop the
    /// armed state); `Ok(false)` = nothing to disarm (e.g. a sovereign-only
    /// enrollment a server "not enrolled" must never touch). Shared by the
    /// explicit-disable, cross-device event, and launch-reconcile paths.
    DisarmComplete(Result<bool, String>, u64),
}

/// Outcome of a server `disable_duress()` call, classified so the panel can
/// branch on the 423 "currently active" case without re-parsing the error body.
#[derive(Debug, Clone)]
pub enum DuressDisableError {
    /// 423 — duress is currently active and can't be disabled.
    Active,
    /// Any other failure; carries a user-facing message.
    Failed(String),
}

impl std::fmt::Debug for DuressMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use DuressMessage::*;
        match self {
            // Sensitive — never print the typed secret.
            RecoveryPassphraseChanged(_) => write!(f, "RecoveryPassphraseChanged(<redacted>)"),
            DuressPinChanged(_) => write!(f, "DuressPinChanged(<redacted>)"),
            DuressPinConfirmChanged(_) => write!(f, "DuressPinConfirmChanged(<redacted>)"),
            AllClearChanged(_) => write!(f, "AllClearChanged(<redacted>)"),
            CrkPasswordChanged(_) => write!(f, "CrkPasswordChanged(<redacted>)"),
            DisablePinChanged(_) => write!(f, "DisablePinChanged(<redacted>)"),
            // Non-sensitive — `ClearResult`/`EnrollResult` carry only an error
            // string (no secret), so they format normally.
            SubmitClear => write!(f, "SubmitClear"),
            ClearResult(res, gen) => write!(f, "ClearResult({:?}, {})", res, gen),
            ForgotAllClear => write!(f, "ForgotAllClear"),
            FinishRecovery => write!(f, "FinishRecovery"),
            StartEnrollment => write!(f, "StartEnrollment"),
            CancelEnrollment => write!(f, "CancelEnrollment"),
            EnrollBack => write!(f, "EnrollBack"),
            EnrollNext => write!(f, "EnrollNext"),
            DelaySelected(d) => write!(f, "DelaySelected({:?})", d),
            MemorizedToggled(b) => write!(f, "MemorizedToggled({})", b),
            SubmitEnrollment => write!(f, "SubmitEnrollment"),
            EnrollPreflightDone(res, gen) => {
                write!(f, "EnrollPreflightDone({:?}, {})", res, gen)
            }
            EnrollResult(res, gen) => write!(f, "EnrollResult({:?}, {})", res, gen),
            CubesLoaded(cubes, gen, seq) => write!(
                f,
                "CubesLoaded({}, {}, {})",
                cubes
                    .as_ref()
                    .map_or_else(|| "<none>".to_string(), |c| format!("{} cubes", c.len())),
                gen,
                seq
            ),
            ReloadCubes => write!(f, "ReloadCubes"),
            StateLoaded(s, gen) => write!(f, "StateLoaded({:?}, {})", s, gen),
            EnrollmentPersisted => write!(f, "EnrollmentPersisted"),
            DisableStart => write!(f, "DisableStart"),
            // Only which method was chosen — never the `CubeSettings` inside
            // `Passkey`. That is a whole settings struct: the passkey credential
            // id, the master-seed fingerprint, and the Cube's random duress
            // marker filename, which is the only thing that locates the marker
            // on disk. This impl is the redaction boundary for duress messages,
            // so a struct dump through it is exactly what it exists to stop.
            DisableMethodProbed(Ok(method), gen) => {
                let method = match method {
                    crate::app::DuressStepUpMethod::Pin => "Pin",
                    crate::app::DuressStepUpMethod::Passkey(_) => "Passkey",
                    crate::app::DuressStepUpMethod::Unavailable => "Unavailable",
                };
                write!(f, "DisableMethodProbed(Ok({}), {})", method, gen)
            }
            // The error is a user-facing string carrying no secret, so it
            // formats normally — same as every other `Result`-bearing arm here.
            DisableMethodProbed(Err(e), gen) => {
                write!(f, "DisableMethodProbed(Err({:?}), {})", e, gen)
            }
            DisableCancel => write!(f, "DisableCancel"),
            DisableSubmit => write!(f, "DisableSubmit"),
            DisablePasskeySubmit => write!(f, "DisablePasskeySubmit"),
            DisableStepUpDone(res, gen) => write!(f, "DisableStepUpDone({:?}, {})", res, gen),
            DisableResult(res, gen) => write!(f, "DisableResult({:?}, {})", res, gen),
            DisarmComplete(res, gen) => write!(f, "DisarmComplete({:?}, {})", res, gen),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContactsMessage {
    /// Contacts list loaded.
    ContactsLoaded(Vec<crate::services::coincube::Contact>, u64),
    /// Invites list loaded.
    InvitesLoaded(Vec<crate::services::coincube::Invite>, u64),
    /// Received-invites list loaded (invites addressed to the current
    /// user). Carries `session_generation` for stale-response guarding,
    /// matching the existing `ContactsLoaded` / `InvitesLoaded` pattern.
    ReceivedInvitesLoaded(Vec<crate::services::coincube::ReceivedInvite>, u64),
    /// User tapped Accept on a received-invite row.
    AcceptReceivedInvite(u64),
    /// Result of a successful accept. Carries the invite id so the row
    /// can be removed optimistically before the contacts refetch lands.
    ReceivedInviteAccepted(u64),
    /// Accept request failed. Removes the id from the in-flight set so
    /// the button re-enables, then surfaces the error via the contacts
    /// `error` field. Plain `Error(String)` would also surface the
    /// message but wouldn't know which invite to clear from the
    /// in-flight set, leaving its Accept button stuck on "Accepting…".
    AcceptReceivedInviteFailed(u64, String),
    /// Navigate to invite form.
    ShowInviteForm,
    /// Navigate back to list.
    BackToList,
    /// Navigate to contact detail.
    ShowDetail(u64),
    /// Email input changed (invite form).
    InviteEmailChanged(String),
    /// Submit invite.
    SubmitInvite,
    /// Invite created successfully — reload list.
    InviteCreated,
    /// Resend a pending invite.
    ResendInvite(u64),
    /// Invite resent successfully.
    InviteResent(u64),
    /// Revoke a pending invite.
    RevokeInvite(u64),
    /// Invite revoked successfully.
    InviteRevoked(u64),
    /// Contact detail cubes loaded — includes contact_id and session_generation to guard against stale responses.
    ContactCubesLoaded(u64, Vec<crate::services::coincube::ContactCube>, u64),
    /// Contact detail cubes fetch failed — includes contact_id for stale guard.
    ContactCubesFailed(u64, String),
    // --- W12: cube multi-select on invite form ---
    /// Available cubes loaded for the invite form (from `list_cubes`).
    /// Carries `session_generation` for stale-response guarding.
    InviteCubesAvailable(Vec<crate::app::state::connect::InviteCubeOption>, u64),
    /// User toggled a cube checkbox in the invite form.
    ToggleInviteCube(u64),
    /// A cube id from the last submit was 403'd by the backend (W12).
    /// Triggers an "unavailable cubes" dialog and reloads the cube list.
    InviteCubeForbidden(String),
    // --- W14: add-existing-contact-to-cube ---
    /// Open the multi-select "Add to Cube(s)…" dialog for an existing
    /// contact. Kicks off the candidate-cube fetch.
    OpenAddToCubeDialog(u64 /* contact id */),
    /// Candidate cubes loaded for the dialog (after network filter,
    /// unjoined filter, and owner-or-member filter).
    AddToCubeCandidatesLoaded(
        u64, /* contact id */
        Vec<crate::app::state::connect::InviteCubeOption>,
        u64, /* session generation */
    ),
    /// User toggled a cube checkbox in the dialog.
    ToggleAddToCubeSelection(u64 /* cube id */),
    /// User confirmed the dialog — fires `create_cube_invite` per
    /// selection.
    ConfirmAddToCube,
    /// Result of the parallel `create_cube_invite` calls. Carries the
    /// originating contact id and session generation so late responses
    /// are dropped instead of landing on a stale or unrelated dialog.
    /// The payload lists per-cube outcomes so the handler can
    /// distinguish full success from partial failure.
    AddToCubeResult(
        u64, /* contact id */
        u64, /* session generation */
        Vec<(u64, Result<(), String>)>,
    ),
    /// Close the dialog without submitting.
    CloseAddToCubeDialog,
    /// One-click "Add to Current Cube" on a contact row. Fires a
    /// single `create_cube_invite` for the active cube.
    AddContactToCurrentCube(u64 /* contact id */),
    /// Result of the one-click add. `Ok(cube_id)` for success,
    /// `Err((contact_id, msg))` for failure.
    AddContactToCurrentCubeResult(u64 /* contact id */, Result<u64, String>),
    /// Error.
    Error(String),
}

/// Per-Cube Connect messages (Lightning Address, Avatar).
#[derive(Debug, Clone)]
pub enum ConnectCubeMessage {
    /// Apply whole-tree focus for a keyboard-reachable Members action.
    FocusAction(iced::advanced::widget::Id),
    LnUsernameChanged(String),
    LnUsernameChecked {
        available: bool,
        error_message: Option<String>,
        version: u32,
    },
    ClaimLightningAddress,
    /// Give a Cube with no Lightning address a random one (e.g.
    /// `orangezebra`): checked free, reserved, registered and confirmed like
    /// a manual claim. Only runs once the Cube's (empty) address is known;
    /// after a failure only an explicit `retry` runs it again.
    AutoAssignLightningAddress {
        retry: bool,
    },
    LightningAddressAutoAssigned {
        generation: u64,
        address: crate::services::coincube::LightningAddress,
    },
    LightningAddressAutoAssignFailed {
        generation: u64,
        error: String,
    },
    LightningAddressClaimed {
        generation: u64,
        address: crate::services::coincube::LightningAddress,
    },
    LightningAddressClaimFailed {
        generation: u64,
        error: String,
    },
    LightningAddressLoaded {
        generation: u64,
        address: Option<crate::services::coincube::LightningAddress>,
    },
    /// Flip the claimed-address card into the in-place edit form.
    BeginEditLightningAddress,
    /// Drop edit mode, clear the input, abort any pending availability check.
    CancelEditLightningAddress,
    /// "Save new address" on Spark › Settings › Lightning address: validates
    /// the new username, then fires the PUT + SDK delete+register chain. The
    /// form states what happens to the old address before Save, so there is
    /// no confirmation step.
    SaveLightningAddress,
    /// Terminal result of the change chain. The outcome variants
    /// distinguish server-side rejection (address unchanged) from
    /// server-committed-but-SDK-out-of-sync (the new address is
    /// confirmed in the API and the existing re-registration prompt
    /// is surfaced for the user to retry the SDK side).
    LightningAddressUpdated {
        generation: u64,
        outcome: crate::app::state::connect::cube::LightningAddressChangeOutcome,
    },
    /// Terminal result of the manual SDK rebind retry kicked off by
    /// `RetryLightningAddressReregister`. `Ok` clears the
    /// re-registration prompt; `Err` keeps it surfaced with an
    /// updated message.
    LightningAddressReregistered(Result<coincube_spark_protocol::LightningAddressInfo, String>),
    /// Manual retry of the SDK side of a Lightning Address rebind
    /// when reconcile flagged `ln_reconcile_needs_reregister`. Performs
    /// SDK delete (idempotent) then register against the DB-confirmed
    /// username.
    RetryLightningAddressReregister,
    /// Phase 4g: the Spark SDK forwarded a `LightningAddressChanged`
    /// event — either a register/unregister on this device, or a
    /// cross-device sync replay via realtime-sync. `None` with a
    /// populated DB reservation triggers auto-re-register.
    SparkLightningAddressChanged(Option<coincube_spark_protocol::LightningAddressInfo>),
    /// Phase 4g: outcome of the startup auto-reconcile. The
    /// [`crate::app::state::connect::cube::ReconcileOutcome`]
    /// payload distinguishes "already in sync / fixed it" from a
    /// transient query failure and from a persistent API↔SDK
    /// divergence that needs the user's attention.
    LightningAddressReconciled(crate::app::state::connect::cube::ReconcileOutcome),
    /// Result of registering the cube with the backend (POST /connect/cubes).
    /// `generation` stamps the authenticated session the attempt was issued
    /// under, so a reply that outlives a sign-out can be discarded rather than
    /// applied to the next account (see `ConnectCubePanel::session_generation`).
    CubeRegistered {
        generation: u64,
        result: Result<
            crate::services::coincube::CubeResponse,
            crate::app::state::connect::cube::CubeRegistrationError,
        >,
    },
    /// Retry a previously failed cube registration.
    RetryRegistration,
    /// Outcome of publishing the Cube's Connect-blinding encryption pubkey
    /// (`PUT /connect/cubes/{id}/encryption-key`, `PLAN-connect-blinding` PR
    /// D2). `false` means the call failed and will be retried next launch —
    /// background hygiene, never surfaced to the user.
    EncryptionKeyRegistered(bool),
    /// Outcome of asserting this Vault's descriptor fingerprint
    /// (`PATCH /connect/cubes/{id}/vault`,
    /// `plans/PLAN-vault-identity-unification.md` D3/D4). `false` means the
    /// call failed and will be retried next launch — background hygiene, never
    /// surfaced to the user.
    VaultFingerprintAsserted(bool),
    /// Outcome of the COIN-373 Vault-membership reconcile at Cube open
    /// (`crate::services::coincube::vault_reconcile`). Carries the number of
    /// member rows attached — background hygiene, never surfaced to the user.
    VaultMembersReconciled(usize),
    CopyToClipboard(String),
    Error(String),
    Avatar(AvatarMessage),
    /// Cube-scoped members management (W8 — see
    /// `plans/PLAN-cube-membership-desktop.md`).
    Members(ConnectCubeMembersMessage),
}

/// Messages for the cube-scoped Members panel. Carries a `load_gen` token on
/// async results so stale responses (e.g. from a prior `Reload`) can be
/// discarded.
#[derive(Debug, Clone)]
pub enum ConnectCubeMembersMessage {
    /// Enter the panel — fires `Reload` if state is empty.
    Enter,
    /// Fetch `GET /connect/cubes/{id}` to refresh members + pending invites.
    Reload,
    Loaded(
        Result<crate::services::coincube::CubeResponse, String>,
        u32, // load generation
    ),
    InviteEmailChanged(String),
    SubmitInvite,
    InviteResult(Result<crate::services::coincube::CubeInviteOrAddResult, String>),
    RevokeInvite(u64),
    RevokeInviteResult(u64, Result<(), String>),
    RemoveMember(u64),
    RemoveMemberResult(u64, Result<(), String>),
    DismissError,
    DismissRemoveConflict,
}

#[derive(Debug, Clone)]
pub enum AvatarMessage {
    /// Enter the Avatar sub-menu — triggers GET /avatar load.
    Enter,
    /// Result of GET /api/v1/connect/avatar.
    Loaded(Result<crate::services::coincube::GetAvatarData, String>),
    /// Navigate to a specific avatar flow step.
    SetStep(crate::app::state::connect::AvatarFlowStep),
    // Questionnaire field changes
    GenderChanged(crate::services::coincube::AvatarGender),
    ArchetypeChanged(crate::services::coincube::AvatarArchetype),
    AgeFeelChanged(crate::services::coincube::AvatarAgeFeel),
    DemeanorChanged(crate::services::coincube::AvatarDemeanor),
    ArmorStyleChanged(crate::services::coincube::AvatarArmorStyle),
    AccentMotifChanged(crate::services::coincube::AvatarAccentMotif),
    LaserEyesToggled(bool),
    /// Submit the questionnaire — triggers POST /avatar/generate.
    Generate,
    /// Result of POST /api/v1/connect/avatar/generate.
    GenerateComplete(Result<crate::services::coincube::AvatarGenerateData, String>),
    /// User picks a variant from the gallery.
    SelectVariant(u64),
    /// Result of POST /api/v1/connect/avatar/select.
    VariantSelected(Result<crate::services::coincube::AvatarSelectData, String>),
    /// Result of GET /api/v1/connect/avatar/regenerations.
    RegenerationsLoaded(Result<crate::services::coincube::RegenerationData, String>),
    /// PNG bytes fetched for a variant.
    ImageLoaded {
        variant_id: u64,
        result: Result<Vec<u8>, String>,
    },
    /// Retry after a generation error.
    Retry,
    /// User pressed Download — save active variant PNG to disk.
    DownloadAvatar,
    /// File-save failed after the user picked a destination.
    SaveError(String),
    /// No-op — used as a return message for tasks that don't need state changes.
    Noop,
}

#[derive(Debug, Clone)]
pub enum HomeMessage {
    /// Toggle only the Overview aggregate; the global privacy setting still wins.
    ToggleTotalBalance,
    /// Toggle only the Overview wallet cards; the global privacy setting still wins.
    ToggleWalletBalances,
    /// Navigate to Liquid Send with asset preset.
    SendAsset(crate::app::state::liquid::send::SendAsset),
    /// Navigate to Liquid Receive with asset preset.
    ReceiveAsset(crate::app::state::liquid::send::SendAsset),
    /// Navigate to Spark Send.
    SendSparkBtc,
    /// Navigate to Spark Receive.
    ReceiveSparkBtc,
    /// Bridge returned a fresh Spark balance (used by the Home
    /// page's periodic balance refresh). Carries the raw
    /// `balance_sats` and the optional Stable Balance USDB holding
    /// so the handler can fold USDB into the headline at the
    /// current BTC/USD price (same pattern as the Spark Overview
    /// panel — when Stable Balance is on, raw `balance_sats` reads
    /// 0 even though the wallet still has spendable value).
    SparkBalanceUpdated {
        btc: Amount,
        usdb_balance: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
        identity_pubkey: String,
    },
    /// Open the wallet-picker popup on the amount screen, editing the named side.
    OpenWalletPicker(PickerSide),
    /// Close the wallet-picker popup without changing the selected pair.
    CloseWalletPicker,
    /// Commit a wallet selection from the popup; the state layer applies it to
    /// the side currently being edited and recomputes `transfer_direction`.
    SelectWalletInPicker(WalletKind),
    /// Choose a source or destination directly from the Transfer form.
    SelectTransferWallet(PickerSide, WalletKind),
    /// The To list's "Another Cube…" row (Spark or Vault source).
    SelectAnotherCube,
    /// Wallets page → Amount, or → Other Cube for "Another Cube…".
    ContinueFromWallets,
    /// Other Cube page: choose a row of the local catalog by Cube id.
    SelectOtherCube(String),
    SelectOtherCubeWallet(WalletKind),
    /// Other Cube page → Unlock.
    ContinueWithCube,
    /// The chosen Cube's 4-digit PIN fields.
    OtherCubePin(crate::pin_input::Message),
    /// Check the entered PIN against the chosen Cube.
    UnlockOtherCube,
    /// Start the chosen Cube's passkey ceremony (macOS).
    UnlockOtherCubePasskey,
    /// Poll the passkey ceremony on the UI thread.
    OtherCubePasskeyPoll,
    OtherCubePinChecked(crate::app::transfer_destination_loader::PinCheck),
    OtherCubeBackendBound(crate::app::transfer_destination_loader::BackendReply),
    OtherCubeAddress(crate::app::transfer_destination_loader::AddressReply),
    OtherCubeSparkAddress(crate::app::transfer_destination_loader::SparkAddressReply),
    /// Unlock page → Amount, once every check passed.
    ContinueFromUnlock,
    /// Amount page: send the source wallet's whole balance, fees included.
    TransferMax,
    /// Done page: close the dialog and open Activity.
    OpenTransferActivity,
    /// Done page: start another transfer.
    RestartTransfer,
    AmountEdited(String),
    NextStep,
    PreviousStep,
    /// A transfer failed. Typed rather than a `String` so the card can say
    /// which wallet is unwell and whether anything was sent — see
    /// [`TransferError`].
    Error(TransferError),
    LiquidBalanceUpdated(Amount),
    /// A dashboard balance refresh failed. Separate from [`Self::Error`]
    /// because it is not a transfer: it fires on every home-screen reload, and
    /// routing it through the transfer copy told users "nothing was sent" when
    /// they had not tried to send anything.
    LiquidBalanceFetchFailed(String),
    UsdtBalanceUpdated(u64),
    UsdtBalanceFetchFailed,
    OnChainLimitsFetched {
        send: (u64, u64),    // (min_sat, max_sat)
        receive: (u64, u64), // (min_sat, max_sat)
    },
    /// The user edited the feerate input on the Transfer confirm screen
    /// (only rendered for Vault-sourced directions — `direction.from_kind() == Vault`).
    SetTransferFeerate(String),
    /// The user clicked a Fast/Normal/Slow preset on the confirm screen's
    /// feerate picker. The handler kicks off a mempool fee-estimate fetch
    /// and applies the result via `TransferFeerateEstimated`.
    FetchTransferFeeratePreset(crate::app::view::shared::feerate_picker::FeeratePreset),
    /// Result of the async preset-driven fee-estimate fetch. On success the
    /// state handler updates `transfer_feerate` to the estimated value.
    TransferFeerateEstimated {
        preset: crate::app::view::shared::feerate_picker::FeeratePreset,
        result: Result<u32, String>,
    },
    /// Result of a dry-run PSBT build for Vault-sourced transfers, used to
    /// show Fees/Total on the confirm screen before signing. Fired whenever
    /// the destination address, feerate, or amount change. `feerate_vb`
    /// echoes the input so the handler can drop late results whose feerate
    /// no longer matches the current state (keystroke-fast previews would
    /// otherwise flicker out-of-order).
    TransferPsbtPreviewReady {
        generation: u64,
        direction: TransferDirection,
        feerate_vb: u64,
        /// The fee and what the destination receives.
        result: Result<(Amount, Amount), String>,
    },
    /// Destination fetch result bound to the request and direction that started it.
    TransferDestinationReady {
        generation: u64,
        direction: TransferDirection,
        result: Result<
            (
                String,
                coincube_core::miniscript::bitcoin::bip32::ChildNumber,
            ),
            TransferError,
        >,
    },
    /// Failure from the same request; old failures must not disturb a newer transfer.
    TransferDestinationFailed {
        generation: u64,
        direction: TransferDirection,
        error: TransferError,
    },
    /// Result of the async step-1→2 prep for Spark-sourced transfers: we
    /// fetched a fresh Vault address (for SparkToVault) or a Breez peg-in
    /// address (for SparkToLiquid) and called `spark.prepare_send(addr, amt)`
    /// on it. Handler stores the destination address + prepare handle so
    /// the confirm screen can render it and `ConfirmSparkSend` can broadcast.
    SparkPrepareSendReady {
        generation: u64,
        direction: TransferDirection,
        /// The destination address Spark will send to. For SparkToVault this
        /// is a fresh Vault BIP-32 address; for SparkToLiquid it's Breez's
        /// on-chain peg-in swap address.
        destination: String,
        /// Single-use handle returned by `spark.prepare_send`.
        prepare_handle: String,
        /// Spark's estimated on-chain fee for this send, in sats. Rendered in
        /// the Fees row on the Transfer confirm screen.
        fee_sat: u64,
    },
    /// Broadcast a previously-prepared Spark send.
    /// Calls `spark.send_payment(handle)` and transitions the pending transfer
    /// to `PendingDeposit` on success.
    ConfirmSparkSend,
    /// Broadcast result for the preparation that the user confirmed.
    SparkTransferFinished {
        generation: u64,
        direction: TransferDirection,
        amount_sat: u64,
        result: Result<(), TransferError>,
    },
    /// A transfer's broadcast step has completed — advance to the Pending
    /// Deposit success screen and mark the destination wallet's pending
    /// indicator. Fired from:
    ///   - `spark.send_payment` success
    ///   - `breez.pay_onchain` success (LiquidToSpark — the LiquidToVault path
    ///     still uses the richer `LiquidToVaultSubmitted` for swap persistence)
    ///   - Vault PSBT broadcast success routed here for VaultToSpark
    TransferBroadcast {
        amount_sat: u64,
        destination_kind: WalletKind,
        /// Breez peg-out swap id when this broadcast came from `pay_onchain`
        /// (currently only LiquidToSpark). Stored against the Spark pending
        /// indicator so an async `PaymentFailed` for the same swap can clear
        /// the Spark card's badge — without it, a failed LiquidToSpark leaves
        /// the badge stuck permanently because no Spark deposit ever arrives.
        swap_id: Option<String>,
    },
    PrepareOnChainResponseReceived {
        generation: u64,
        direction: TransferDirection,
        response: PreparePayOnchainResponse,
    },
    TransferSuccessful,
    BackToHome,
    RefreshLiquidBalance,
    RefreshSparkBalance,
    /// Bridge has emitted `Event::Synced`. The next (or already
    /// in-flight) Spark `get_info` response can be trusted — until
    /// this fires, the value the SDK returns may be whatever it
    /// persisted from a previous session, before incremental sync.
    SparkSyncedObserved,
    /// Tick from the fallback poll subscription that runs while the
    /// Spark card is in its loading state. Bumps the retry counter
    /// and re-fetches the balance; force-releases the loading UI if
    /// the counter hits its cap without ever observing `Synced`.
    /// Distinct from `RefreshSparkBalance` (which is dispatched on
    /// every SparkEvent) so SDK event bursts don't burn through the
    /// retry budget.
    SparkLoadRetry,
    /// `load_spark_balance`'s `get_info` RPC failed (bridge unreachable
    /// or timed out). Clears the in-flight guard so a subsequent
    /// refresh / retry can fire. Soft-fail: the previously displayed
    /// balance, if any, is left intact.
    SparkLoadFailed,
    SignVaultToLiquidTx,
    TransferPsbtReady {
        request_id: u64,
        generation: u64,
        direction: TransferDirection,
        destination: String,
        amount_sat: u64,
        feerate_vb: u64,
        result: TransferPsbtResult,
    },
    TransferSigningComplete {
        request_id: u64,
        generation: u64,
        direction: TransferDirection,
    },
    ConfirmTransfer,
    LiquidToVaultSubmitted {
        amount: Amount,
        swap_id: Option<String>,
    },
    LiquidToVaultPending(Option<String>),
    LiquidToVaultWaitingConfirmation(Option<String>),
    LiquidToVaultSucceeded(Option<String>),
    LiquidToVaultFailed(Option<String>),
    PendingTransferRestored {
        amount_sat: u64,
        stage: TransferStage,
        swap_id: String,
    },
    PendingAmountsUpdated {
        liquid_send_sats: u64,
        usdt_send_sats: u64,
        liquid_receive_sats: u64,
        usdt_receive_sats: u64,
    },
    /// Fired by the SideShift-swap flow the moment its shift settles: bitcoin
    /// is now en route to the Spark wallet's static on-chain deposit address.
    /// The Home state registers it as a pending incoming Spark deposit so the
    /// existing auto-claim watcher claims it on maturity and fires the
    /// "received" celebration once it lands. `amount_sat` is the swap's settle
    /// estimate, used only for the pending-indicator amount (the celebration
    /// uses the actual claimed amount).
    SwapSettledAwaitingArrival {
        amount_sat: u64,
    },
    /// Fired when the Spark bridge reports `DepositsChanged`. The Home state
    /// re-queries `list_unclaimed_deposits` and decides whether to auto-claim
    /// a matured deposit or clear `pending_spark_incoming` (claimed already).
    SparkDepositsChanged,
    /// Result of the Home state's own `list_unclaimed_deposits` call.
    SparkDepositsLoaded(Vec<coincube_spark_protocol::DepositInfo>),
    /// Completion signal for the auto-claim call. On success, another
    /// `DepositsChanged` event will fire and the watcher re-runs.
    ///
    /// `Ok(None)` is a claim the SDK accepted before maturity (0.25.0+): the
    /// transfer settles asynchronously and its settled amount is unknown
    /// until [`Self::SparkPaymentSucceeded`] reports it.
    AutoClaimSparkResult {
        txid: String,
        vout: u32,
        result: Result<Option<u64>, String>,
    },
    /// The Spark bridge reported `PaymentSucceeded` for an incoming transfer
    /// of `amount_sat`. Home only acts on it while an auto-claim is waiting
    /// for its asynchronous settlement (see [`Self::AutoClaimSparkResult`]).
    SparkPaymentSucceeded {
        amount_sat: u64,
    },
    /// Fired when a Breez peg-in swap completes (BTC on-chain → L-BTC).
    /// The state handler decrements `pending_liquid_receive_sats` and re-runs
    /// `load_pending_sends` for full self-heal.
    LiquidPeginCompleted {
        amount_sat: u64,
    },
}

#[derive(Debug, Clone)]
pub enum P2PMessage {
    OrderTypeSelected(super::p2p::components::OrderType),
    PricingModeSelected(super::p2p::components::PricingMode),
    FiatCurrencyEdited(String),
    SatsAmountEdited(String),
    PremiumEdited(String),
    PaymentMethodSelected(String),
    PaymentMethodRemoved(String),
    CustomPaymentMethodEdited(String),
    AddCustomPaymentMethod,
    MinAmountEdited(String),
    MaxAmountEdited(String),
    RangeOrderToggled(bool),
    LightningAddressEdited(String),
    EditLightningAddress,
    UseRegisteredLightningAddress,
    SubmitOrder,
    ClearForm,
    MostroOrdersReceived(Vec<super::p2p::components::P2POrder>),
    BuySellFilterChanged(super::p2p::components::BuySellFilter),
    FilterCurrencySelected(String),
    FilterPaymentMethodToggled(String),
    FilterMinRatingChanged(f32),
    FilterMinDaysActiveChanged(u32),
    SelectOrder(String),
    CloseOrderDetail,
    CopyOrderId(String),
    CancelOrder(String),
    CancelOrderResult(Result<(), String>),
    OrderSubmitResult(Result<super::p2p::mostro::OrderSubmitResponse, String>),
    TradeFilterChanged(super::p2p::components::TradeFilter),
    MostroTradesReceived(Vec<super::p2p::components::P2PTrade>),
    // Mostro settings
    MostroRelayInputEdited(String),
    MostroAddRelay,
    MostroRemoveRelay(String),
    MostroNodeNameInputEdited(String),
    MostroNodePubkeyInputEdited(String),
    /// Toggle whether the node being added is a test-network coordinator
    /// (`true`) or a mainnet coordinator (`false`).
    MostroNodeNetworkToggled(bool),
    MostroAddNode,
    MostroRemoveNode(String),
    MostroSelectActiveNode(String),
    MostroNodeInfoReceived {
        currencies: Vec<String>,
        min_order_sats: Option<u64>,
        max_order_sats: Option<u64>,
    },
    ConfirmOrder,
    CancelConfirmation,
    // Take order flow
    TakeOrder,
    TakeOrderAmountEdited(String),
    TakeOrderInvoiceEdited(String),
    ConfirmTakeOrder,
    CancelTakeOrder,
    TakeOrderResult(Result<super::p2p::mostro::TakeOrderResponse, String>),
    DismissPaymentInvoice,
    CopyPaymentInvoice(String),
    ResetInvoiceCopied,
    CancelPaymentInvoice(String),
    /// Spark balance lookup for the pending-payment modal completed.
    /// Drives the Spark-pay-first UX: when the balance covers the hold
    /// invoice (plus a small fee buffer), the modal opens with a
    /// "Pay from Spark" button instead of a QR code.
    ///
    /// `order_id` identifies the originating Mostro order so a stale
    /// response from a previous session can't mutate state for the
    /// active one — see `P2PPanel::spark_pay_session_id`.
    SparkBalanceLoaded {
        order_id: String,
        balance_sat: u64,
    },
    SparkBalanceFailed {
        order_id: String,
        err: String,
    },
    /// Result of pre-parsing the hold-invoice via `spark.parse_input`
    /// alongside the balance fetch. Carries the BOLT11 amount so the
    /// Spark-pay summary can show "Lock amount: X sats" before the
    /// user clicks "Pay from Spark" — even for market-priced sell
    /// orders where Mostro leaves `trade.sats_amount = None`.
    /// `amount_sat` is `None` when the parse failed or the invoice
    /// carried no amount (an unusual case for Mostro hold invoices).
    SparkInvoiceAmountParsed {
        order_id: String,
        amount_sat: Option<u64>,
    },
    /// User pressed "Pay from Spark" — either in the payment-required
    /// modal (right after taking a buy order) or in the trade-detail
    /// view (after navigating back to a trade with a pending hold
    /// invoice). `order_id` scopes the resulting prepare/send chain
    /// so a dismissed session can't pay against a fresh modal.
    SparkPayPrepare {
        order_id: String,
        invoice: String,
    },
    /// `prepare_send` succeeded — preview is ready (amount + fee).
    SparkPayPrepared {
        order_id: String,
        ok: coincube_spark_protocol::PrepareSendOk,
    },
    /// User confirmed the Spark-pay preview. Triggers `send_payment`.
    SparkPayConfirm,
    /// `send_payment` finished successfully — dismiss the modal.
    SparkPaySent {
        order_id: String,
        ok: coincube_spark_protocol::SendPaymentOk,
    },
    /// Any Spark prepare/send step failed. Stays in the modal so the
    /// user can retry or fall through to the QR.
    SparkPayFailed {
        order_id: String,
        err: String,
    },
    /// Abandon the in-progress Spark pay (drops a prepared handle, returns
    /// to the Spark-pay idle button).
    SparkPayCancel,
    /// Flip the "Pay from another wallet" toggle. `true` reveals the
    /// QR/Copy Invoice body, `false` returns to Spark-pay mode.
    ToggleQrFallback(bool),
    // Trade detail
    SelectTrade(String),
    CloseTradeDetail,
    // Trade actions
    SubmitInvoice,
    TradeInvoiceEdited(String),
    ConfirmFiatSent,
    ConfirmFiatReceived,
    RatingSelected(u8),
    SubmitRating,
    CancelTrade,
    OpenDispute,
    TradeActionResult(Result<super::p2p::mostro::TradeActionResponse, String>),
    // Real-time DM updates
    TradeUpdate {
        order_id: String,
        action: String,
        payload_json: String,
    },
    // Timer tick for trade detail countdown
    TradeTimerTick,
    // Chat
    OpenChat,
    CloseChat,
    ChatInputEdited(String),
    SendChatMessage,
    ChatMessageSent(Result<(), String>),
    ToggleChatTradeInfo,
    ToggleChatUserInfo,
    // Dispute chat
    OpenDisputeChat,
    CloseDisputeChat,
    DisputeChatInputEdited(String),
    SendDisputeChatMessage,
    DisputeChatMessageSent(Result<(), String>),
    // Chat list
    ChatListTabMessages,
    ChatListTabDisputes,
    OpenChatForTrade(String),
    OpenDisputeChatForTrade(String),
    // File attachments
    AttachFile,
    FileSelected(std::path::PathBuf),
    /// (order_id, metadata_json) on success, error string on failure.
    AttachmentSent(Result<(String, String), String>),
    AttachmentDownloaded {
        order_id: String,
        blossom_url: String,
        data: Result<Vec<u8>, String>,
    },
    SaveFile {
        blossom_url: String,
        filename: String,
    },
    FileSaved(Result<(), String>),
    // Stream-level errors (relay connection, subscription, restore failures)
    StreamError(String),
}

#[cfg(test)]
mod recovery_kit_upload_outcome_debug_tests {
    use super::{RecoveryKitMessage, RecoveryKitUploadOutcome};

    /// Canary string embedded into the fingerprint. If it ever appears in
    /// a Debug render we know the redaction regressed.
    const CANARY_FP: &str = "canary-fp-XYZZY-do-not-leak";

    fn outcome_with_fp(fp: Option<String>) -> RecoveryKitUploadOutcome {
        RecoveryKitUploadOutcome {
            updated_at: "2026-04-22T00:00:00Z".to_string(),
            now_has_seed: true,
            now_has_descriptor: true,
            descriptor_fingerprint: fp,
        }
    }

    #[test]
    fn debug_redacts_some_fingerprint_but_preserves_presence() {
        let outcome = outcome_with_fp(Some(CANARY_FP.to_string()));
        let rendered = format!("{:?}", outcome);
        assert!(
            !rendered.contains(CANARY_FP),
            "fingerprint hex leaked through Debug: {}",
            rendered
        );
        assert!(
            rendered.contains("<redacted>"),
            "redaction marker missing: {}",
            rendered
        );
        assert!(
            rendered.contains("Some"),
            "presence of Some() should remain visible: {}",
            rendered
        );
        assert!(rendered.contains("updated_at"));
        assert!(rendered.contains("now_has_seed"));
        assert!(rendered.contains("now_has_descriptor"));
    }

    #[test]
    fn debug_renders_none_when_fingerprint_absent() {
        let outcome = outcome_with_fp(None);
        let rendered = format!("{:?}", outcome);
        assert!(
            rendered.contains("None"),
            "absent case lost signal: {}",
            rendered
        );
        assert!(!rendered.contains(CANARY_FP));
    }

    #[test]
    fn upload_result_ok_debug_does_not_leak_fingerprint() {
        let msg =
            RecoveryKitMessage::UploadResult(Ok(outcome_with_fp(Some(CANARY_FP.to_string()))));
        let rendered = format!("{:?}", msg);
        assert!(
            !rendered.contains(CANARY_FP),
            "Debug(RecoveryKitMessage::UploadResult(Ok(..))) leaked fingerprint: {}",
            rendered
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn phone_seal_result_ok_debug_does_not_leak_fingerprint() {
        // The keychain drift fingerprint (PR 3) is the same class of stable
        // cross-session identifier as the upload outcome's — redact its hex.
        let msg = RecoveryKitMessage::PhoneSealResult(Ok(Some(CANARY_FP.to_string())));
        let rendered = format!("{:?}", msg);
        assert!(
            !rendered.contains(CANARY_FP),
            "Debug(RecoveryKitMessage::PhoneSealResult(Ok(..))) leaked fingerprint: {}",
            rendered
        );
        assert!(rendered.contains("<redacted>"));
    }
}

#[cfg(test)]
mod duress_message_debug_tests {
    use super::DuressMessage;

    /// Canary string; if it ever shows up in a Debug render the redaction has
    /// regressed and a typed PIN/passphrase would leak into logs.
    const CANARY: &str = "canary-secret-XYZZY-do-not-leak";

    #[test]
    fn sensitive_variants_are_redacted() {
        let sensitive = [
            DuressMessage::RecoveryPassphraseChanged(CANARY.to_string()),
            DuressMessage::DuressPinChanged(CANARY.to_string()),
            DuressMessage::DuressPinConfirmChanged(CANARY.to_string()),
            DuressMessage::AllClearChanged(CANARY.to_string()),
            DuressMessage::CrkPasswordChanged(CANARY.to_string()),
            DuressMessage::DisablePinChanged(CANARY.to_string()),
        ];
        for msg in &sensitive {
            let rendered = format!("{:?}", msg);
            assert!(
                !rendered.contains(CANARY),
                "Debug({:?}) leaked the secret payload",
                rendered
            );
            assert!(rendered.contains("<redacted>"), "got {}", rendered);
        }
    }

    #[test]
    fn non_sensitive_variants_format_normally() {
        assert_eq!(
            format!("{:?}", DuressMessage::SubmitEnrollment),
            "SubmitEnrollment"
        );
        assert_eq!(
            format!("{:?}", DuressMessage::MemorizedToggled(true)),
            "MemorizedToggled(true)"
        );
    }

    /// The probed method names itself and nothing else. `DuressStepUpMethod`
    /// derives `Debug`, so rendering the `Result` wholesale would print the
    /// entire `CubeSettings` inside `Passkey` — including `duress_slot_file`,
    /// the random filename that is the only way to locate this Cube's duress
    /// marker on disk.
    #[test]
    fn a_probed_passkey_method_does_not_print_the_cube() {
        const MARKER: &str = "canary-marker-XYZZY.mnemonic";

        let mut cube = crate::app::settings::CubeSettings::new_with_id(
            uuid::Uuid::nil(),
            "Passkey Cube".to_string(),
            coincube_core::miniscript::bitcoin::Network::Bitcoin,
        );
        cube.duress_slot_file = Some(MARKER.to_string());

        let rendered = format!(
            "{:?}",
            DuressMessage::DisableMethodProbed(
                Ok(crate::app::DuressStepUpMethod::Passkey(Box::new(cube))),
                7,
            )
        );
        assert!(
            !rendered.contains(MARKER),
            "the duress marker filename reached a log line: {}",
            rendered
        );
        assert_eq!(rendered, "DisableMethodProbed(Ok(Passkey), 7)");
    }

    /// The other two methods and the failure still say what happened — the
    /// redaction must not cost the generation or the error text, which is what
    /// makes these lines worth logging at all.
    #[test]
    fn the_other_probe_outcomes_still_carry_their_detail() {
        assert_eq!(
            format!(
                "{:?}",
                DuressMessage::DisableMethodProbed(Ok(crate::app::DuressStepUpMethod::Pin), 3)
            ),
            "DisableMethodProbed(Ok(Pin), 3)"
        );
        assert_eq!(
            format!(
                "{:?}",
                DuressMessage::DisableMethodProbed(
                    Ok(crate::app::DuressStepUpMethod::Unavailable),
                    4,
                )
            ),
            "DisableMethodProbed(Ok(Unavailable), 4)"
        );
        assert_eq!(
            format!(
                "{:?}",
                DuressMessage::DisableMethodProbed(Err("settings unreadable".to_string()), 5)
            ),
            "DisableMethodProbed(Err(\"settings unreadable\"), 5)"
        );
    }
}
