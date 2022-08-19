use crate::app::menu::Menu;

#[derive(Debug, Clone)]
pub enum Message {
    Reload,
    Clipboard(String),
    Menu(Menu),
}
