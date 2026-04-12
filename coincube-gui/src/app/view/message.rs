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
    services::{
        fiat::{Currency, PriceSource},
        sideshift::{ShiftQuote, ShiftResponse, ShiftStatus, SideshiftNetwork},
    },
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
}

#[derive(Debug, Clone)]
pub enum Message {
    Scroll(f32),
    Reload,
    Clipboard(String),
    Menu(Menu),
    ToggleVault,
    ToggleLiquid,
    ToggleMarketplace,
    ToggleMarketplaceP2P,
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
    SetAssetFilter(crate::app::state::liquid::transactions::AssetFilter),
    ShowError(String),
    ShowSuccess(String),
    ShowToast(log::Level, String),
    DismissToast(usize),
    SideshiftReceive(SideshiftReceiveMessage),
    SideshiftSend(SideshiftSendMessage),
    ConnectAccount(ConnectAccountMessage),
    ConnectCube(ConnectCubeMessage),
    ToggleConnect,
    P2P(P2PMessage),
    ToggleTheme,
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

#[derive(Debug, Clone)]
pub enum LiquidOverviewMessage {
    SendLbtc,
    ReceiveLbtc,
    SendUsdt,
    ReceiveUsdt,
    History,
    SelectTransaction(usize),
    DataLoaded {
        balance: Amount,
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
    UsdtAmountEdited(String),
}

#[derive(Debug, Clone)]
pub enum LiquidReceiveMessage {
    ToggleMethod(ReceiveMethod),
    Copy,
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
        recent_payment: Vec<breez_sdk_liquid::prelude::Payment>,
    },
    /// User tapped a recent transaction row.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,
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
    BackupWallet(BackupWalletMessage),
    SettingsUpdated,
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
    VerifiedDevicesLoaded(Vec<crate::services::coincube::VerifiedDevice>, u64),
    LoginActivityLoaded(Vec<crate::services::coincube::LoginActivity>, u64),
    CopyToClipboard(String),
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
    /// Result of registering the cube with the backend (POST /connect/cubes).
    CubeRegistered(Result<crate::services::coincube::CubeResponse, String>),
    /// Retry a previously failed cube registration.
    RetryRegistration,
    CopyToClipboard(String),
    Error(String),
    Avatar(AvatarMessage),
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
    /// Navigate to Send with asset preset.
    SendAsset(crate::app::state::liquid::send::SendAsset),
    /// Navigate to Receive with asset preset.
    ReceiveAsset(crate::app::state::liquid::send::SendAsset),
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
    LightningAddressEdited(String),
    SubmitOrder,
    ClearForm,
    MostroOrdersReceived(Vec<super::p2p::components::P2POrder>),
    BuySellFilterChanged(super::p2p::components::BuySellFilter),
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
