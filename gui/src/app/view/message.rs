use crate::app::menu::Menu;

#[derive(Debug, Clone)]
pub enum Message {
    Reload,
    Clipboard(String),
    Menu(Menu),
    Close,
    Select(usize),
    Settings(usize, SettingsMessage),
    CreateSpend(CreateSpendMessage),
    Next,
    Previous,
}

#[derive(Debug, Clone)]
pub enum CreateSpendMessage {
    AddRecipient,
    DeleteRecipient(usize),
    SelectCoin(usize),
    RecipientEdited(usize, &'static str, String),
    FeerateEdited(String),
    Generate,
    Save,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Edit,
    FieldEdited(&'static str, String),
    CancelEdit,
    ConfirmEdit,
}
