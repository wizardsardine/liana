use crate::{
    app::{
        menu::Menu,
        view::{global_home::TransferDirection, FiatAmountConverter},
    },
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};

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
    ActiveSend(ActiveSendMessage),
    ActiveReceive(ActiveReceiveMessage),
    ActiveSettings(ActiveSettingsMessage),
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
    CountryDetected(Result<crate::services::coincube::Country, String>),

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
}

#[derive(Debug, Clone)]
pub enum FiatMessage {
    Enable(bool),
    SourceEdited(PriceSource),
    CurrencyEdited(Currency),
}

#[derive(Debug, Clone)]
pub enum ActiveSendMessage {
    InvoiceEdited(String),
    Send,
    ViewHistory,
}

#[derive(Debug, Clone)]
pub enum ActiveReceiveMessage {
    ToggleMethod(ReceiveMethod),
    Copy,
    GenerateAddress,
    AddressGenerated(ReceiveMethod, Result<String, String>),
    AmountInput(String),
    DescriptionInput(String),
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
    fn from(msg: FiatMessage) -> Self {
        Message::Settings(SettingsMessage::Fiat(msg))
    }
}

#[derive(Debug, Clone)]
pub enum HomeMessage {
    ToggleBalanceMask,
    SelectTransferDirection(TransferDirection),
    AmountEdited(String),
    ConfirmTransfer,
    NextStep,
    PreviousStep,
}
