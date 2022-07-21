use std::path::PathBuf;

use super::Error;

#[derive(Debug, Clone)]
pub enum Message {
    Event(iced_native::Event),
    Exit(PathBuf),
    Next,
    Previous,
    Install,
    Installed(Result<PathBuf, Error>),
    Network(bitcoin::Network),
    DefineBitcoind(DefineBitcoind),
    DefineDescriptor(DefineDescriptor),
}

#[derive(Debug, Clone)]
pub enum DefineBitcoind {
    CookiePathEdited(String),
    AddressEdited(String),
}

#[derive(Debug, Clone)]
pub enum DefineDescriptor {
    ImportDescriptor(String),
    UserXpubEdited(String),
    HeirXpubEdited(String),
    SequenceEdited(String),
}
