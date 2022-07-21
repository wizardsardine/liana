use minisafe::config::Config as DaemonConfig;

use crate::app::{error::Error, menu::Menu};

#[derive(Debug, Clone)]
pub enum Message {
    Reload,
    Tick,
    Event(iced_native::Event),
    Clipboard(String),
    Menu(Menu),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
}
