use crate::{
    app::{
        menu::Menu,
        settings::unit::BitcoinDisplayUnit,
        view::{
            global_home::{IncomingTransferStage, TransferDirection},
            FiatAmountConverter,
        },
    },
    export::ImportExportMessage,
    node::bitcoind::{Bitcoind, RpcAuthType},
    services::fiat::{Currency, PriceSource},
};
use coincubed::config::BitcoindConfig;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FeeratePriority {
    Low,
    Medium,
    High,
}

use breez_sdk_liquid::prelude::{
    InputType, Payment, PreparePayOnchainResponse, PrepareSendResponse,
};
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
    ClearToast,
}

#[derive(Debug, Clone)]
pub enum Message {
    Scroll(f32),
    Reload,
    Clipboard(String),
    Menu(Menu),
    ToggleVault,
    ToggleLiquid,
    SetupVault,
    Close,
    Select(usize),
    SelectRefundable(usize),
    RefundAddressEdited(String),
    RefundAddressValidated(bool),
    RefundFeerateEdited(String),
    RefundFeeratePrioritySelected(FeeratePriority),
    SubmitRefund,
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
    LiquidReceive(LiquidReceiveMessage),
    VaultReceive(VaultReceiveMessage),
    LiquidSend(LiquidSendMessage),
    LiquidSettings(LiquidSettingsMessage),
    PreselectPayment(Payment),
    ShowError(String),
    DismissToast(usize),
    UsdtOverview(UsdtOverviewMessage),
    ToggleUsdt,
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
    Broadcast,
    Save,
    Confirm,
    Cancel,
    SelectHotSigner,
    EditPsbt,
    PsbtEdited(String),
    Next,
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
}

#[derive(Debug, Clone)]
pub enum InstallStatsViewMessage {
    PeriodChanged(crate::services::coincube::StatsPeriod),
    Refresh,
}

#[derive(Debug, Clone)]
pub enum NodeSettingsMessage {
    SwitchToConnect,
    SwitchToBitcoind,
    // COINCUBE | Connect re-authentication sub-flow (gates the Switch to Connect action)
    ConnectLoginEmailChanged(String),
    ConnectLoginRequestOtp,
    ConnectLoginOtpRequested(Result<(), String>),
    ConnectLoginOtpChanged(String),
    ConnectLoginVerifyOtp,
    ConnectLoginVerified(Result<String, String>), // Ok(jwt_token)
    ConnectLoginCancel,
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

#[derive(Debug, Clone)]
pub enum LiquidOverviewMessage {
    SendLbtc,
    ReceiveLbtc,
    History,
    SelectTransaction(usize),
    DataLoaded {
        balance: Amount,
        recent_payment: Vec<Payment>,
    },
    Error(String),
    RefreshRequested,
}

#[derive(Debug, Clone)]
pub enum UsdtOverviewMessage {
    SendUsdt,
    ReceiveUsdt,
    History,
    SelectTransaction(usize),
    DataLoaded {
        usdt_balance: u64,
        recent_payment: Vec<Payment>,
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
        recent_payment: Vec<Payment>,
    },
    Error(String),
    ClearError,
    // Send flow popup messages
    PopupMessage(SendPopupMessage),
    PrepareResponseReceived(PrepareSendResponse),
    PrepareOnChainResponseReceived(PreparePayOnchainResponse),
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
    UsdtAmountEdited(String),
}

#[derive(Debug, Clone)]
pub enum LiquidReceiveMessage {
    ToggleMethod(ReceiveMethod),
    Copy,
    ClearToast,
    GenerateAddress,
    AddressGenerated(ReceiveMethod, Result<String, String>),
    AmountInput(String),
    UsdtAmountInput(String),
    DescriptionInput(String),
    Error(String),
    ClearError,
    OnChainLimitsFetched { min_sat: u64, max_sat: u64 },
    LightningLimitsFetched { min_sat: u64, max_sat: u64 },
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
    BackupWallet(BackupWalletMessage),
    SettingsUpdated,
    ExportPayments,
}

#[derive(Debug, Clone)]
pub enum BackupWalletMessage {
    ToggleBackupIntroCheck,
    Start,
    NextStep,
    PreviousStep,
    VerifyPhrase,
    Complete,
    WordInput { index: u8, input: String },
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

#[derive(Debug, Clone)]
pub enum HomeMessage {
    ToggleBalanceMask,
    SelectTransferDirection(TransferDirection),
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
    PrepareOnChainResponseReceived(PreparePayOnchainResponse),
    TransferSuccessful,
    BackToHome,
    BreezOnchainAddress(String),
    RefreshLiquidBalance,
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
        stage: IncomingTransferStage,
        swap_id: String,
    },
    PendingTransferAnimationTick,
    PendingAmountsUpdated {
        liquid_send_sats: u64,
        usdt_send_sats: u64,
        liquid_receive_sats: u64,
        usdt_receive_sats: u64,
    },
}
