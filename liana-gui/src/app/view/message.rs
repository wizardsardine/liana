use crate::{
    app::menu::Menu,
    app::view::FiatAmountConverter,
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};

#[cfg(feature = "buysell")]
use crate::services::mavapay::{PriceResponse, QuoteResponse, Transaction};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountType {
    Individual,
    Business,
}

#[derive(Debug, Clone)]
pub enum CreateRbfMessage {
    New(bool),
    FeerateEdited(String),
    Cancel,
    Confirm,
}

// TODO(Option B): Consider splitting BuySellMessage into sub-enums for clearer flow separation:
// - BuySellMessage::Shared(SharedMsg)
// - BuySellMessage::Africa(AfricaMsg)
// - BuySellMessage::International(InternationalMsg)
// This would reduce unrelated match arms and better reflect the runtime flow_state boundaries.
#[cfg(feature = "buysell")]
#[derive(Debug, Clone)]
pub enum BuySellMessage {
    // Native login (default build)
    LoginUsernameChanged(String),
    LoginPasswordChanged(String),
    SubmitLogin,
    CreateAccountPressed,

    // Default build: account type selection
    AccountTypeSelected(AccountType),
    GetStarted,

    // Geolocation detection
    CountryDetected(String, String), // (country_name, iso_code)

    // Default build: registration form (native flow)
    FirstNameChanged(String),
    LastNameChanged(String),
    EmailChanged(String),
    Password1Changed(String),
    Password2Changed(String),
    TermsToggled(bool),
    SubmitRegistration,
    CheckEmailVerificationStatus,
    ResendVerificationEmail,
    RegistrationSuccess,
    RegistrationError(String),
    EmailVerificationStatusChecked(bool),
    EmailVerificationStatusError(String),
    ResendEmailSuccess,
    ResendEmailError(String),
    LoginSuccess(crate::services::registration::LoginResponse),
    LoginError(String),

    // Mavapay-specific messages (native flow)
    MavapayDashboard,
    MavapayFlowModeChanged(crate::app::view::buysell::flow_state::MavapayFlowMode),
    MavapayAmountChanged(String),
    MavapaySourceCurrencyChanged(String),
    MavapayTargetCurrencyChanged(String),
    MavapaySettlementCurrencyChanged(String),
    MavapayPaymentMethodChanged(crate::app::view::buysell::flow_state::MavapayPaymentMethod),
    MavapayBankAccountNumberChanged(String),
    MavapayBankAccountNameChanged(String),
    MavapayBankCodeChanged(String),
    MavapayBankNameChanged(String),
    MavapayCreateQuote,
    MavapayOpenPaymentLink,
    MavapayQuoteCreated(QuoteResponse),
    MavapayQuoteError(String),
    MavapayConfirmQuote,
    MavapayGetPrice,
    MavapayPriceReceived(PriceResponse),
    MavapayPriceError(String),
    MavapayGetTransactions,
    MavapayTransactionsReceived(Vec<Transaction>),
    MavapayTransactionsError(String),

    // creates a webview session on onramper
    CreateSession,
    SessionError(String),
    ResetWidget,
    SetBuyOrSell(super::buysell::panel::BuyOrSell),
    SetFlowState(super::buysell::flow_state::BuySellFlowState),
    CreateNewAddress,
    AddressCreated(super::buysell::panel::LabelledAddress),

    // webview messages (gated)
    WebviewOpenUrl(String),
    WryMessage(iced_wry::IcedWryMessage),
    WryExtractedWindowId(iced_wry::ExtractedWindowId),

    // Open external URL in browser
    OpenExternalUrl(String),
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
