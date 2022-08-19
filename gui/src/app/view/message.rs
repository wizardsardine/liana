use crate::app::menu::Menu;

#[derive(Debug, Clone)]
pub enum Message {
    Reload,
    Clipboard(String),
    Menu(Menu),
    Settings(usize, SettingsMessage),
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Edit,
    FieldEdited(&'static str, String),
    CancelEdit,
    ConfirmEdit,
}
