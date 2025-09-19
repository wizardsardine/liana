use crate::{
    app::menu::Menu,
    app::view::FiatAmountConverter,
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};
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
    ExportDescriptor,
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
#[cfg(feature = "buysell")]
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

#[cfg(feature = "buysell")]
#[derive(Debug, Clone)]
pub enum BuySellMessage {
    // Native login (default build)
    LoginUsernameChanged(String),
    LoginPasswordChanged(String),
    SubmitLogin,
    CreateAccountPressed,

    // Default build: account type selection
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    AccountTypeSelected(AccountType),
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    GetStarted,

    // Shared form fields (for provider-integrated builds)
    WalletAddressChanged(String),
    #[cfg(feature = "dev-meld")]
    CountryCodeChanged(String),
    #[cfg(feature = "dev-onramp")]
    FiatCurrencyChanged(String),
    SourceAmountChanged(String),

    CreateSession,
    SessionError(String),

    // webview messages (gated)
    #[cfg(feature = "webview")]
    WebviewCreated(iced_webview::ViewId),
    #[cfg(feature = "webview")]
    ViewTick(iced_webview::ViewId),
    #[cfg(feature = "webview")]
    WebviewAction(iced_webview::advanced::Action),
    #[cfg(feature = "webview")]
    WebviewOpenUrl(String),
    #[cfg(feature = "webview")]
    CloseWebview,
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
