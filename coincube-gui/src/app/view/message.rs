use crate::{
    app::{
        menu::Menu,
        settings::unit::BitcoinDisplayUnit,
        view::{global_home::TransferDirection, FiatAmountConverter},
    },
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};

use breez_sdk_liquid::prelude::{
    InputType, Payment, PreparePayOnchainResponse, PrepareSendResponse,
};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Address, OutPoint};
use coincube_core::spend::SpendCreationError;

pub trait Close {
    fn close() -> Self;
}

#[derive(Debug, Clone)]
pub enum Message {
    Scroll(f32),
    Reload,
    Clipboard(String),
    Menu(Menu),
    ToggleVault,
    ToggleActive,
    SetupVault,
    Close,
    Select(usize),
    SelectPayment(OutPoint),
    Label(Vec<String>, LabelMessage),
    NextReceiveAddress,
    ToggleShowPreviousAddresses,
    SelectAddress(Address),
    Settings(SettingsMessage),
    CreateSpend(CreateSpendMessage),
    ImportSpend(ImportSpendMessage),
    #[cfg(feature = "buysell")]
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
    ActiveOverview(ActiveOverviewMessage),
    ActiveReceive(ActiveReceiveMessage),
    ActiveSend(ActiveSendMessage),
    ActiveSettings(ActiveSettingsMessage),
    PreselectPayment(Payment),
    DismissError,
    ShowError(String),
    DismissErrorIfId(u64),
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

#[cfg(feature = "buysell")]
#[derive(Debug, Clone)]
pub enum BuySellMessage {
    // state management
    SessionError(&'static str, String), // inline description + runtime error
    ResetWidget,
    SelectBuyOrSell(bool), // true = buy, false = sell
    StartSession,
    LogOut,

    // automatic user login
    SubmitLogin {
        skip_email_verification: bool,
    },
    LoginSuccess {
        login: crate::services::coincube::LoginResponse,
        email_verified: bool,
    },

    // ip geolocation
    CountryDetected(Result<crate::services::coincube::Country, crate::services::coincube::CoincubeError>),

    // recipient address generation
    CreateNewAddress,
    AddressCreated(super::buysell::panel::LabelledAddress),

    // user Registration
    LegalNameChanged(String),
    EmailChanged(String),
    Password1Changed(String),
    Password2Changed(String),
    SubmitRegistration,
    RegistrationSuccess,

    // email Verification
    SendVerificationEmail,
    CheckEmailVerificationStatus,
    EmailVerificationFailed,

    // login to coincube account
    LoginUsernameChanged(String),
    LoginPasswordChanged(String),
    CreateNewAccount,
    ResetPassword,

    // Password Reset
    SendPasswordResetEmail,
    PasswordResetEmailSent(String),
    ReturnToLogin,

    // Mavapay specific messages
    Mavapay(crate::services::mavapay::MavapayMessage),

    // Meld specific messages
    Meld(crate::app::view::buysell::meld::MeldMessage),

    // Clipboard action (forwarded to parent Message::Clipboard)
    Clipboard(String),

    ViewHistory,
}

#[cfg(feature = "buysell")]
#[derive(Debug, Clone)]
pub enum WebviewMessage {
    WryMessage(iced_wry::IcedWryMessage),
    InitWryWebviewWithUrl(iced_wry::ExtractedWindowId, String),
}

#[derive(Debug, Clone)]
pub enum FiatMessage {
    Enable(bool),
    SourceEdited(PriceSource),
    CurrencyEdited(Currency),
}

#[derive(Debug, Clone)]
pub enum ActiveOverviewMessage {
    Send,
    Receive,
    History,
    SelectTransaction(usize),
    DataLoaded {
        balance: Amount,
        recent_payment: Vec<Payment>,
    },
    Error(ActiveOverviewError),
    RefreshRequested,
}

#[derive(Debug, Clone)]
pub enum ActiveOverviewError {
    BalanceFetch(String),
    TransactionsFetch(String),
    BalanceAndTransactionsFetch(String, String),
}

impl std::fmt::Display for ActiveOverviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::BalanceFetch(e) => write!(f, "Couldn't fetch account balance: {}", e),
            Self::TransactionsFetch(e) => write!(f, "Couldn't fetch recent transactions: {}", e),
            Self::BalanceAndTransactionsFetch(e1, e2) => {
                write!(f, "Couldn't fetch balance or transactions: {}, {}", e1, e2)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ActiveSendMessage {
    InputEdited(String),
    InputValidated(Option<InputType>),
    Send,
    History,
    SelectTransaction(usize),
    DataLoaded {
        balance: Amount,
        recent_payment: Vec<Payment>,
    },
    Error(ActiveSendError),
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
pub enum ActiveSendError {
    BalanceFetch(String),
    TransactionsFetch(String),
    BalanceAndTransactionsFetch(String, String),
    LightningLimitsFetch(String),
    OnChainLimitsFetch(String),
    PrepareSend(String),
    Send(String),
}

impl std::fmt::Display for ActiveSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::BalanceFetch(e) => write!(f, "Couldn't fetch account balance: {}", e),
            Self::TransactionsFetch(e) => write!(f, "Couldn't fetch recent transactions: {}", e),
            Self::BalanceAndTransactionsFetch(e1, e2) => {
                write!(f, "Couldn't fetch balance or transactions: {}, {}", e1, e2)
            }
            Self::LightningLimitsFetch(e) => write!(f, "Couldn't fetch lightning limits: {}", e),
            Self::OnChainLimitsFetch(e) => write!(f, "Couldn't fetch on-chain limits: {}", e),
            Self::PrepareSend(e) => write!(f, "Failed to prepare send: {}", e),
            Self::Send(e) => write!(f, "Failed to send payment: {}", e),
        }
    }
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
}

#[derive(Debug, Clone)]
pub enum ActiveReceiveMessage {
    ToggleMethod(ReceiveMethod),
    Copy,
    ClearToast,
    GenerateAddress,
    AddressGenerated(ReceiveMethod, Result<String, ReceiveError>),
    AmountInput(String),
    DescriptionInput(String),
    Error(String),
    ClearError,
}

#[derive(Debug, Clone)]
pub enum ReceiveError {
    LightningInvoice(String),
    OnChainAddress(String),
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::LightningInvoice(e) => write!(f, "Failed to generate Lightning invoice: {}", e),
            Self::OnChainAddress(e) => write!(f, "Failed to generate Bitcoin address: {}", e),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveMethod {
    Lightning,
    OnChain,
}

#[derive(Debug, Clone)]
pub enum ActiveSettingsMessage {
    BackupWallet(BackupWalletMessage),
    SettingsUpdated,
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
    ActiveBalanceUpdated(Amount),
    OnChainLimitsFetched {
        send: (u64, u64),    // (min_sat, max_sat)
        receive: (u64, u64), // (min_sat, max_sat)
    },
    PrepareOnChainResponseReceived(PreparePayOnchainResponse),
    TransferSuccessful,
    BackToHome,
    BreezOnchainAddress(String),
    RefreshActiveBalance,
    SignVaultToActiveTx,
    TransferPsbtReady(
        Result<
            (
                coincube_core::miniscript::bitcoin::psbt::Psbt,
                Option<std::sync::Arc<crate::app::wallet::Wallet>>,
                (
                    crate::dir::CoincubeDirectory,
                    coincube_core::miniscript::bitcoin::Network,
                ),
            ),
            String,
        >,
    ),
    TransferSigningComplete,
    ConfirmTransfer,
}
