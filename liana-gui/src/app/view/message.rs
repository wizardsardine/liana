use crate::{
    app::menu::Menu,
    app::view::FiatAmountConverter,
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};

#[cfg(feature = "buysell")]
use crate::services::mavapay::GetPriceResponse;
use liana::miniscript::bitcoin::{bip32::Fingerprint, Address, OutPoint};

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
    SessionError(String),
    ResetWidget,
    SetBuyOrSell(super::buysell::panel::BuyOrSell),
    StartOnramperSession,

    // recipient address generation
    CreateNewAddress,
    AddressCreated(super::buysell::panel::LabelledAddress),

    // Geolocation detection
    ManualCountrySelected(crate::services::geolocation::Country),
    CountryDetected(Result<(String, String), String>),

    // webview logic
    WebviewOpenUrl(String),
    WryMessage(iced_wry::IcedWryMessage),
    StarWryWebviewWithUrl(iced_wry::ExtractedWindowId, String),

    // Mavapay specific messages
    Mavapay(MavapayMessage),
}

#[cfg(feature = "buysell")]
#[derive(Debug, Clone)]
pub enum MavapayMessage {
    LoginSuccess(crate::services::coincube::LoginResponse),
    // User Registration
    FirstNameChanged(String),
    LastNameChanged(String),
    EmailChanged(String),
    Password1Changed(String),
    Password2Changed(String),
    SubmitRegistration,
    RegistrationSuccess,
    // Email Verification
    SendVerificationEmail,
    CheckEmailVerificationStatus,
    EmailVerificationFailed,
    // login to existing mavapay account
    LoginUsernameChanged(String),
    LoginPasswordChanged(String),
    SubmitLogin,
    CreateNewAccount,
    // buysell flow
    FlowModeChanged(crate::app::view::buysell::flow_state::MavapayFlowMode),
    AmountChanged(u64),
    SourceCurrencyChanged(crate::services::mavapay::MavapayUnitCurrency),
    TargetCurrencyChanged(crate::services::mavapay::MavapayUnitCurrency),
    SettlementCurrencyChanged(crate::services::mavapay::MavapayCurrency),
    PaymentMethodChanged(crate::services::mavapay::MavapayPaymentMethod),
    BankAccountNumberChanged(String),
    BankAccountNameChanged(String),
    BankCodeChanged(String),
    BankNameChanged(String),
    CreateQuote,
    OpenPaymentLink,
    GetPrice,
    PriceReceived(GetPriceResponse),
}

#[derive(Debug, Clone)]
pub enum FiatMessage {
    Enable(bool),
    SourceEdited(PriceSource),
    CurrencyEdited(Currency),
}

impl From<FiatMessage> for Message {
    fn from(msg: FiatMessage) -> Self {
        Message::Settings(SettingsMessage::Fiat(msg))
    }
}
