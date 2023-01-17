use liana::miniscript::bitcoin::{util::bip32::Fingerprint, Network};
use std::path::PathBuf;

use super::Error;
use crate::hw::HardwareWallet;

#[derive(Debug, Clone)]
pub enum Message {
    CreateWallet,
    ImportWallet,
    BackupDone(bool),
    Exit(PathBuf),
    Clibpboard(String),
    Next,
    Previous,
    Install,
    Close,
    Reload,
    Select(usize),
    Installed(Result<PathBuf, Error>),
    Network(Network),
    DefineBitcoind(DefineBitcoind),
    DefineDescriptor(DefineDescriptor),
    ConnectedHardwareWallets(Vec<HardwareWallet>),
    WalletRegistered(Result<(Fingerprint, Option<[u8; 32]>), Error>),
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
