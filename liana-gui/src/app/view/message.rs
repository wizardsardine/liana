use liana_ui::{
    component::panels::{map::header::HeaderAction, spend::FeeLevel},
    widget::graph_view::{GraphEvent, ItemId},
};

use crate::{
    app::{menu::Menu, settings::WalletId, view::FiatAmountConverter},
    export::ImportExportMessage,
    node::bitcoind::RpcAuthType,
    services::fiat::{Currency, PriceSource},
};
use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address, OutPoint,
};

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
    NewAddress(NewAddressMessage),
    ToggleShowPreviousAddresses,
    ToggleHideConfirmedPsbts,
    Settings(SettingsMessage),
    CreateSpend(CreateSpendMessage),
    Spend(SpendTxMessage),
    Next,
    Previous,
    SelectHardwareWallet(usize),
    CreateRbf(CreateRbfMessage),
    ShowAddressQrCode(AddressQrSource),
    ShowQrOptSection(bool),
    ImportExport(ImportExportMessage),
    HideRescanWarning,
    ExportPsbt,
    ImportPsbt,
    OpenUrl(String),
    Map(MapMessage),
}

impl Close for Message {
    fn close() -> Self {
        Self::Close
    }
}

#[derive(Debug, Clone)]
pub enum MapMessage {
    Header(HeaderAction),
    Graph(GraphEvent),
    CloseModal,
    ToggleCoinSelected,
    ToggleFrozen,
    ToggleTagPopover,
    TagFilterEdited(String),
    /// Tag registry index.
    TagToggled(usize),
    TagCreate,
    ClearCoinSelection,
    Key(MapKey),
    /// Leaf to jump to.
    ReuseRowSelected(ItemId),
    /// Adds or removes another wallet from the map.
    WalletToggled(WalletId),
    /// Opens the external wallet import form.
    ImportWallet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapKey {
    Undo,
    Redo,
    Shortcuts,
    Unspent,
    Escape,
    /// Escape taken by a focused text input.
    EscapeInInput,
    Command(bool),
}

#[derive(Debug, Clone)]
pub enum LabelMessage {
    Edit,
    Edited(String),
    Cancel,
    Confirm,
}

#[derive(Debug, Clone)]
pub enum AddressQrSource {
    Row(usize),                      // QR omits the derivation index
    WithIndex(Address, ChildNumber), // specter DIY: QR includes the index
}

#[derive(Debug, Clone)]
pub enum NewAddressMessage {
    LabelEdited(String),
    Confirm,
    Verify,
    ShowQr,
    Close,
}

#[derive(Debug, Clone)]
pub enum CreateSpendMessage {
    AddRecipient,
    TxLabelEdited(String),
    DeleteRecipient(usize),
    SelfTransfer,
    SelectCoin(usize),
    RecipientEdited(usize, &'static str, String),
    RecipientFiatAmountEdited(usize, String, FiatAmountConverter),
    FeerateEdited(String),
    FeeModeManual,
    FeeModeSmart,
    SelectFeeLevel(FeeLevel),
    SelectPath(usize),
    Generate,
    SendMaxToRecipient(usize),
    Clear,
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
}

#[allow(clippy::large_enum_variant)]
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

impl From<SettingsMessage> for Message {
    fn from(value: SettingsMessage) -> Self {
        Message::Settings(value)
    }
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
