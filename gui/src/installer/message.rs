use minisafe::miniscript::bitcoin;
use std::path::PathBuf;

use super::Error;
use crate::hw::HardwareWallet;

#[derive(Debug, Clone)]
pub enum Message {
    Event(iced_native::Event),
    Exit(PathBuf),
    Next,
    Previous,
    Install,
    Close,
    Reload,
    Select(usize),
    Installed(Result<PathBuf, Error>),
    Network(bitcoin::Network),
    DefineBitcoind(DefineBitcoind),
    DefineDescriptor(DefineDescriptor),
    ConnectedHardwareWallets(Vec<HardwareWallet>),
}

#[derive(Debug, Clone)]
pub enum DefineBitcoind {
    CookiePathEdited(String),
    AddressEdited(String),
}

#[derive(Debug, Clone)]
pub enum DefineDescriptor {
    ImportDescriptor(String),
    ImportUserHWXpub,
    ImportHeirHWXpub,
    XpubImported(Result<String, Error>),
    UserXpubEdited(String),
    HeirXpubEdited(String),
    SequenceEdited(String),
}
