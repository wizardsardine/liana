use crate::{
    app::{
        menu::Menu,
        settings::unit::BitcoinDisplayUnit,
        view::{
            global_home::{PickerSide, TransferStage, WalletKind},
            FiatAmountConverter,
        },
    },
    export::ImportExportMessage,
    node::bitcoind::{Bitcoind, RpcAuthType},
    services::{
        fiat::{Currency, PriceSource},
        sideshift::{ShiftQuote, ShiftResponse, ShiftStatus, SideshiftNetwork},
    },
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

#[derive(Debug, Clone)]
pub enum VaultReceiveMessage {
    Copy(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Scroll(f32),
    Reload,
    Clipboard(String),
    Menu(Menu),
    SetupVault,
    /// W15 — launch the vault installer in "restore from Connect
    /// Recovery Kit" mode. Branches off the same button the default
    /// `SetupVault` goes through, but flags the installer to use
    /// `UserFlow::RestoreVaultFromRecoveryKit` instead of
    /// `CreateWallet`.
    SetupVaultRestoreFromKit,
    Close,
    Select(usize),
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
    CreateRbf(CreateRbfMessage),
    ShowQrCode(usize),
    ImportExport(ImportExportMessage),
    HideRescanWarning,
    ExportPsbt,
    ImportPsbt,
    OpenUrl(String),
    Home(HomeMessage),
    LiquidOverview(LiquidOverviewMessage),
    LiquidTransactions(LiquidTransactionsMessage),
    SparkOverview(crate::app::view::spark::SparkOverviewMessage),
    SparkTransactions(crate::app::view::spark::SparkTransactionsMessage),
    SparkSettings(crate::app::view::spark::SparkSettingsMessage),
    SparkSend(crate::app::view::spark::SparkSendMessage),
    SparkReceive(crate::app::view::spark::SparkReceiveMessage),
    LiquidReceive(LiquidReceiveMessage),
    VaultReceive(VaultReceiveMessage),
    LiquidSend(LiquidSendMessage),
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
    DismissToast(usize),
    SideshiftReceive(SideshiftReceiveMessage),
    SideshiftSend(SideshiftSendMessage),
    ConnectAccount(ConnectAccountMessage),
    ConnectCube(ConnectCubeMessage),
    P2P(P2PMessage),
    /// Bubbles up from a Connect-requiring feature page (Spark
    /// Settings → Lightning Address, Cube Settings → Avatar / Members)
    /// when an unauthenticated user clicks "Sign In" on the inline
    /// prompt. The Pane intercepts it and focuses the Home tab on its
    /// Connect section.
    OpenConnectSignIn,
    ToggleTheme,
    DismissReceivedCelebration,
    DismissBackupWarning,
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
    /// Open the Keychain signing flow: contact-signers and self-signers
    /// whose private keys live on a Connect-registered phone. Routed to
    /// `KeychainSignModal` which fetches vault members, classifies the
    /// required signers, and orchestrates `SigningSession` lifecycles
    /// via gRPC.
    SignKeychain,
    /// Cancel all non-terminal `SigningSession`s tracked by the open
    /// `KeychainSignModal`. Discards any partial signatures already
    /// returned — those are not merged into the PSBT until all signers
    /// have responded.
    CancelKeychainSign,
    /// Retry a single signer whose session expired or was rejected.
    /// Carries the per-signer position in `KeychainSignModal::pending`.
    RetryKeychainSigner(usize),
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

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    EditBitcoindSettings,
    BitcoindSettings(SettingsEditMessage),
    ElectrumSettings(SettingsEditMessage),
    RescanSettings(SettingsEditMessage),
    ImportExport(ImportExportMessage),
    EditRemoteBackendSettings,
    RemoteBackendSettings(RemoteBackendSettingsMessage),
    EditWalletSettings,
    ImportExportSection,
    ExportEncryptedDescriptor,
    ExportPlaintextDescriptor,
    ExportTransactions,
    ExportLabels,
    ExportWallet,
    ImportWallet,
    AboutSection,
    /// Triggered by the "Re-register this device" button on the
    /// Settings → About page's Connect-device card. Clears the cached
    /// `device_id` and re-runs `ensure_device_registered`, getting a
    /// fresh `SignerDevice` row server-side. Useful when a device id
    /// is suspect (compromise, machine swap) or the row went stale on
    /// the API and signing started 404'ing.
    ReregisterConnectDevice,
    /// Settled result of the re-registration RPC. Payload is the new
    /// device_id on success, or a user-facing error string. Routed
    /// straight to the About settings state so it can refresh its
    /// banner without going through the catchall update path.
    ConnectDeviceReregistered(Result<String, String>),
    RegisterWallet,
    FingerprintAliasEdited(Fingerprint, String),
    WalletAliasEdited(String),
    Save,
    GeneralSection,
    DisplayUnitChanged(BitcoinDisplayUnit),
    Fiat(FiatMessage),
    NodeSettings(NodeSettingsMessage),
    InstallStatsSection,
    InstallStats(InstallStatsViewMessage),
    TestToast(log::Level),
    ToggleDirectionBadges(bool),
    /// Master seed backup flow (moved from Liquid Settings to Cube/General Settings).
    BackupMasterSeed(BackupWalletMessage),
    BackupMasterSeedUpdated,
    /// Cube Recovery Kit — Connect-hosted encrypted backup of the seed
    /// and/or wallet descriptor. See `PLAN-cube-recovery-kit-desktop.md`.
    /// Coexists with `BackupMasterSeed` above (the local paper-phrase
    /// backup), it does not replace it.
    RecoveryKit(RecoveryKitMessage),
    /// Local LAN signer ("Paired phones") section — pairs a Keychain
    /// phone over the local network so it shows up in the signer
    /// list. See `PLAN-local-signer-lan-desktop.md`.
    LocalSigningSection,
    LocalSigning(LocalSigningMessage),
}

#[derive(Debug, Clone)]
pub enum LocalSigningMessage {
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
    RemovePhone(String),
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
    // "Set up local node while on Connect" sub-flow
    SetupLocalNode,
    SetupLocalNodeCancel,
    SetupLocalNodeAddrChanged(String),
    SetupLocalNodeAuthTypeSelected(RpcAuthType),
    SetupLocalNodeFieldEdited(&'static str, String),
    SetupLocalNodeConfirm,
    // Mode picker: false = self-managed external, true = COINCUBE-managed internal
    SetupLocalNodeModeSelected(bool),
    // Internal (COINCUBE-managed) node setup progress
    SetupLocalNodeDownloadProgress(f32),
    SetupLocalNodeDownloadComplete(Result<Vec<u8>, String>),
    SetupLocalNodeStartResult(Result<(BitcoindConfig, Bitcoind), String>),
}

#[derive(Debug, Clone)]
pub enum RemoteBackendSettingsMessage {
    EditInvitationEmail(String),
    SendInvitation,
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
    StatusUpdated(Result<ShiftStatus, String>),
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
    History,
    SelectTransaction(usize),
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
    PresetAsset(crate::app::state::liquid::send::SendAsset),
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
    PrepareResponseReceived(PrepareSendResponse),
    PrepareOnChainResponseReceived(PreparePayOnchainResponse),
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
    ToggleMethod(ReceiveMethod),
    Copy,
    ShowQrCode,
    CloseQrCode,
    DismissCelebration,
    GenerateAddress,
    AddressGenerated(ReceiveMethod, Result<String, String>),
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
    PinVerified(Result<Vec<String>, String>),
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
    /// User clicked Remove. Fires `delete_recovery_kit`.
    Remove,
    /// Async result of Remove.
    RemoveResult(Result<(), String>),
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
            Self::PasswordChanged(_) => f
                .debug_tuple("PasswordChanged")
                .field(&"<redacted>")
                .finish(),
            Self::ConfirmChanged(_) => f
                .debug_tuple("ConfirmChanged")
                .field(&"<redacted>")
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

/// Account-level Connect messages (login/session, plan, security, etc.).
#[derive(Debug, Clone)]
pub enum ConnectAccountMessage {
    Init,
    RefreshSession {
        refresh_token: String,
    },
    SetSession(crate::services::coincube::LoginResponse),
    SessionLoaded {
        user: crate::services::coincube::User,
        plan: Option<crate::services::coincube::ConnectPlan>,
    },
    PlanLoaded(Option<crate::services::coincube::ConnectPlan>, u64),
    RefreshFailed(String),
    LogOut,
    EmailChanged(String),
    SubmitLogin,
    SubmitRegistration,
    CreateAccount,
    OtpRequested {
        email: String,
        is_signup: bool,
    },
    OtpChanged(String),
    OtpCooldownTick,
    ResendOtp,
    OtpResent,
    VerifyOtp,
    EmailNotVerified {
        email: String,
    },
    VerifiedDevicesLoaded(Vec<crate::services::coincube::VerifiedDevice>, u64),
    LoginActivityLoaded(Vec<crate::services::coincube::LoginActivity>, u64),
    CopyToClipboard(String),
    Contacts(ContactsMessage),
    Error(String),
    // --- Plan & Billing ---
    FeaturesLoaded(Option<crate::services::coincube::FeaturesResponse>, u64),
    BillingCycleSelected(crate::services::coincube::BillingCycle),
    StartCheckout(crate::services::coincube::PlanTier),
    CheckoutCreated(
        Result<crate::services::coincube::CheckoutResponse, String>,
        u64,
    ),
    PollChargeStatus,
    ChargeStatusUpdated(
        Result<crate::services::coincube::ChargeStatusResponse, (String, bool)>,
        u64,
    ),
    DismissCheckout,
    OpenCheckoutUrl(String),
    BillingHistoryLoaded(
        Result<Vec<crate::services::coincube::BillingHistoryEntry>, String>,
        u64,
    ),
    ToggleBillingHistory,
    /// User profile refreshed (billing history view update)
    UserProfileLoaded(crate::services::coincube::User),
    /// User profile refresh failed (non-auth error)
    UserProfileFailed(String),
    // --- Duress (Phases 6 & 8) ---
    /// Result of the post-sign-in `get_duress_state` gate. When `active`,
    /// the panel replaces the dashboard with the recovery flow.
    DuressStateChecked(Option<crate::services::coincube::DuressState>, u64),
    /// Recovery flow + enrollment wizard messages (nested to keep this enum
    /// tidy).
    Duress(DuressMessage),
}

/// Messages for the duress recovery flow (Phase 6) and enrollment wizard
/// (Phases 2 & 8), nested under [`ConnectAccountMessage::Duress`].
#[derive(Debug, Clone)]
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
    StartEnrollment,
    /// Sovereign "Sign up for Connect" CTA → the Register flow.
    SignUpForConnect,
    CancelEnrollment,
    EnrollBack,
    EnrollNext,
    RegularPinChanged(String),
    DuressPinChanged(String),
    AllClearChanged(String),
    CrkPasswordChanged(String),
    DelaySelected(crate::services::duress::enroll::DuressDelay),
    SovereignConfirmChanged(String),
    MemorizedToggled(bool),
    SubmitEnrollment,
    EnrollResult(Result<(), String>, u64),
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
    LnUsernameChanged(String),
    LnUsernameChecked {
        available: bool,
        error_message: Option<String>,
        version: u32,
    },
    ClaimLightningAddress,
    LightningAddressClaimed(crate::services::coincube::LightningAddress),
    LightningAddressLoaded(Option<crate::services::coincube::LightningAddress>),
    /// Flip the claimed-address card into the in-place edit form.
    BeginEditLightningAddress,
    /// Drop edit mode, clear the input, abort any pending availability check.
    CancelEditLightningAddress,
    /// User clicked "Change" on the edit form — open the destructive
    /// confirmation modal with the proposed new username.
    RequestChangeLightningAddress,
    /// User backed out of the confirmation modal.
    DismissChangeConfirmation,
    /// User confirmed the swap — fires the PUT + SDK delete+register chain.
    ConfirmChangeLightningAddress,
    /// Terminal result of the change chain. The outcome variants
    /// distinguish server-side rejection (address unchanged) from
    /// server-committed-but-SDK-out-of-sync (the new address is
    /// confirmed in the API and the existing re-registration prompt
    /// is surfaced for the user to retry the SDK side).
    LightningAddressUpdated(crate::app::state::connect::cube::LightningAddressChangeOutcome),
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
    CubeRegistered(Result<crate::services::coincube::CubeResponse, String>),
    /// Retry a previously failed cube registration.
    RetryRegistration,
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
        stable_balance: Option<coincube_spark_protocol::StableBalanceSnapshot>,
    },
    ToggleBalanceMask,
    /// Open the wallet-picker popup on the amount screen, editing the named side.
    OpenWalletPicker(PickerSide),
    /// Close the wallet-picker popup without changing the selected pair.
    CloseWalletPicker,
    /// Commit a wallet selection from the popup; the state layer applies it to
    /// the side currently being edited and recomputes `transfer_direction`.
    SelectWalletInPicker(WalletKind),
    AmountEdited(String),
    NextStep,
    PreviousStep,
    Error(String),
    LiquidBalanceUpdated(Amount),
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
        feerate_vb: u64,
        result: Result<Amount, String>,
    },
    /// Result of the async step-1→2 prep for Spark-sourced transfers: we
    /// fetched a fresh Vault address (for SparkToVault) or a Breez peg-in
    /// address (for SparkToLiquid) and called `spark.prepare_send(addr, amt)`
    /// on it. Handler stores the destination address + prepare handle so
    /// the confirm screen can render it and `ConfirmSparkSend` can broadcast.
    SparkPrepareSendReady {
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
    /// Broadcast a previously-prepared Spark send (SparkToVault, SparkToLiquid).
    /// Calls `spark.send_payment(handle)` and transitions the pending transfer
    /// to `PendingDeposit` on success.
    ConfirmSparkSend,
    /// A transfer's broadcast step has completed — advance to the Pending
    /// Deposit success screen and mark the destination wallet's pending
    /// indicator. Fired from:
    ///   - `spark.send_payment` success (SparkToVault, SparkToLiquid)
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
    PrepareOnChainResponseReceived(PreparePayOnchainResponse),
    TransferSuccessful,
    BackToHome,
    BreezOnchainAddress(String),
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
    TransferPsbtReady(TransferPsbtResult),
    TransferSigningComplete,
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
    /// Fired when the Spark bridge reports `DepositsChanged`. The Home state
    /// re-queries `list_unclaimed_deposits` and decides whether to auto-claim
    /// a matured deposit or clear `pending_spark_incoming` (claimed already).
    SparkDepositsChanged,
    /// Result of the Home state's own `list_unclaimed_deposits` call.
    SparkDepositsLoaded(Vec<coincube_spark_protocol::DepositInfo>),
    /// Completion signal for the auto-claim call. On success, another
    /// `DepositsChanged` event will fire and the watcher re-runs.
    AutoClaimSparkResult {
        txid: String,
        vout: u32,
        result: Result<u64, String>,
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
}
