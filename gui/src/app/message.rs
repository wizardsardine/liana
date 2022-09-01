use minisafe::config::Config as DaemonConfig;

use crate::app::{error::Error, view};

#[derive(Debug)]
pub enum Message {
    Tick,
    Event(iced_native::Event),
    View(view::Message),
    LoadDaemonConfig(Box<DaemonConfig>),
    DaemonConfigLoaded(Result<(), Error>),
    BlockHeight(Result<i32, Error>),
    ReceiveAddress(Result<bitcoin::Address, Error>),
}
