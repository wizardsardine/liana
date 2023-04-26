use crate::app::menu::Menu;
use liana::miniscript::bitcoin::util::bip32::Fingerprint;

#[derive(Debug, Clone)]
pub enum Message {
    Reload,
    Clipboard(String),
    Menu(Menu),
    Close,
    Select(usize),
    SelectSub(usize, usize),
    Settings(SettingsMessage),
    CreateSpend(CreateSpendMessage),
    ImportSpend(ImportSpendMessage),
    Spend(SpendTxMessage),
    Next,
    Previous,
    SelectHardwareWallet(usize),
}

#[derive(Debug, Clone)]
pub enum CreateSpendMessage {
    AddRecipient,
    DeleteRecipient(usize),
    SelectCoin(usize),
    RecipientEdited(usize, &'static str, String),
    FeerateEdited(String),
    SelectPath(usize),
    Generate,
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
    EditWalletSettings,
    AboutSection,
    RegisterWallet,
    FingerprintAliasEdited(Fingerprint, String),
    Save,
    Edit(usize, SettingsEditMessage),
}

#[derive(Debug, Clone)]
pub enum SettingsEditMessage {
    Select,
    FieldEdited(&'static str, String),
    Cancel,
    Confirm,
}
