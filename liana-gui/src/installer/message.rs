use liana::miniscript::{
    bitcoin::{
        bip32::{ChildNumber, Fingerprint},
        Network,
    },
    DescriptorPublicKey,
};
use std::collections::HashMap;

use super::{context, Error};
use crate::{
    app::{
        settings::{self, ProviderKey},
        view::Close,
    },
    backup::{self, Backup},
    download::{DownloadError, Progress},
    export::ImportExportMessage,
    hw::HardwareWalletMessage,
    installer::descriptor::{Key, PathKind},
    node::{
        bitcoind::{Bitcoind, ConfigField, RpcAuthType},
        electrum, NodeType,
    },
    services::{
        self,
        connect::client::{auth::AuthClient, backend::api},
    },
};

#[derive(Debug, Clone)]
pub enum Message {
    UserActionDone(bool),
    Exit(
        Box<settings::WalletSettings>,
        Option<Bitcoind>,
        /* remove log */ bool,
    ),
    Clibpboard(String),
    Next,
    Skip,
    Previous,
    BackToLauncher(Network),
    Install,
    Close,
    Reload,
    Select(usize),
    UseHotSigner,
    Installed(settings::WalletId, Result<settings::WalletSettings, Error>),
    CreateTaprootDescriptor(bool),
    SelectDescriptorTemplate(context::DescriptorTemplate),
    SelectBackend(SelectBackend),
    ImportRemoteWallet(ImportRemoteWallet),
    SelectBitcoindType(SelectBitcoindTypeMsg),
    InternalBitcoind(InternalBitcoindMsg),
    DefineNode(DefineNode),
    DefineDescriptor(DefineDescriptor),
    ImportXpub(Fingerprint, Result<DescriptorPublicKey, Error>),
    HardwareWallets(HardwareWalletMessage),
    WalletRegistered(Result<(Fingerprint, Option<[u8; 32]>), Error>),
    MnemonicWord(usize, String),
    ImportMnemonic(bool),
    RedeemNextKey,
    KeyRedeemed(ProviderKey, Result<(), services::keys::Error>),
    AllKeysRedeemed,
    BackupWallet,
    ExportWallet(Result<String, backup::Error>),
    ExportXpub(String),
    ImportExport(ImportExportMessage),
    ImportBackup,
    WalletFromBackup((HashMap<Fingerprint, settings::KeySetting>, Backup)),
    WalletAliasEdited(String),
    SelectAccount(Fingerprint, ChildNumber),
}

impl Close for Message {
    fn close() -> Self {
        Self::Close
    }
}

impl From<ImportExportMessage> for Message {
    fn from(value: ImportExportMessage) -> Self {
        Message::ImportExport(value)
    }
}

impl From<(Fingerprint, ChildNumber)> for Message {
    fn from(value: (Fingerprint, ChildNumber)) -> Self {
        Self::SelectAccount(value.0, value.1)
    }
}

#[derive(Debug, Clone)]
pub enum SelectBackend {
    // view messages
    RequestOTP,
    EditEmail,
    EmailEdited(String),
    OTPEdited(String),
    ContinueWithLocalWallet(bool),
    // Commands messages
    OTPRequested(Result<(AuthClient, String), Error>),
    OTPResent(Result<(), Error>),
    Connected(Result<context::RemoteBackend, Error>),
}

#[derive(Debug, Clone)]
pub enum ImportRemoteWallet {
    RemoteWallets(Result<Vec<api::Wallet>, Error>),
    ImportDescriptor(String),
    ConfirmDescriptor,
    ImportInvitationToken(String),
    FetchInvitation,
    InvitationFetched(Result<api::WalletInvitation, Error>),
    AcceptInvitation,
    InvitationAccepted(Result<api::Wallet, Error>),
}

#[derive(Debug, Clone)]
pub enum DefineBitcoind {
    ConfigFieldEdited(ConfigField, String),
    RpcAuthTypeSelected(RpcAuthType),
}

#[derive(Debug, Clone)]
pub enum DefineElectrum {
    ConfigFieldEdited(electrum::ConfigField, String),
}

#[derive(Debug, Clone)]
pub enum DefineNode {
    NodeTypeSelected(NodeType),
    DefineBitcoind(DefineBitcoind),
    DefineElectrum(DefineElectrum),
    PingResult((NodeType, Result<(), Error>)),
    Ping,
}

#[derive(Debug, Clone)]
pub enum SelectBitcoindTypeMsg {
    UseExternal(bool),
}

#[derive(Debug, Clone)]
pub enum InternalBitcoindMsg {
    Previous,
    Reload,
    DefineConfig,
    Download,
    DownloadProgressed(Result<Progress, DownloadError>),
    Install,
    Start,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DefineDescriptor {
    ChangeTemplate(context::DescriptorTemplate),
    ImportDescriptor(String),
    KeysEdited(Vec<(usize, usize)>, Key),
    KeysEdit(PathKind, Vec<(usize, usize)>),
    Path(usize, DefinePath),
    AddRecoveryPath,
    AddSafetyNetPath,
    KeyModal(ImportKeyModal),
    ThresholdSequenceModal(ThresholdSequenceModal),
    Reset,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DefinePath {
    AddKey,
    Key(usize, DefineKey),
    ThresholdEdited(usize),
    SequenceEdited(u16),
    EditSequence,
    EditThreshold,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DefineKey {
    Delete,
    Edit,
    Clipboard(String),
}

#[derive(Debug, Clone)]
pub enum ImportKeyModal {
    FetchedKey(Result<Key, Error>),
    XPubEdited(String),
    NameEdited(String),
    ManuallyImportXpub,
    ConfirmXpub,
    UseToken(services::keys::api::KeyKind),
    TokenEdited(String),
    ConfirmToken,
    SelectKey(usize),
    ImportXpub(Network),
}

#[derive(Debug, Clone)]
pub enum ThresholdSequenceModal {
    ThresholdEdited(usize),
    SequenceEdited(String),
    Confirm,
}
