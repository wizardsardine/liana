use liana::{
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{ChildNumber, Fingerprint},
            Network,
        },
        DescriptorPublicKey,
    },
};
use std::collections::HashMap;

use super::{
    context,
    step::descriptor::editor::key::{EditKeyAliasMessage, SelectKeySourceMessage, SelectedKey},
    Error,
};
use crate::{
    app::{
        settings::{self, ProviderKey},
        view::Close,
    },
    backup::Backup,
    download::{DownloadError, Progress},
    export::ImportExportMessage,
    hw::HardwareWalletMessage,
    installer::{decrypt::Decrypt, descriptor::PathKind},
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
    Exit(Box<settings::WalletSettings>, Option<Bitcoind>),
    Clipboard(String),
    Next,
    Skip,
    Previous,
    BackToLauncher(Network),
    BackToApp(Network),
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
    HardwareWalletUpdate,
    WalletRegistered(Result<(Fingerprint, Option<[u8; 32]>), Error>),
    MnemonicWord(usize, String),
    ImportMnemonic(bool),
    RedeemNextKey,
    KeyRedeemed(ProviderKey, Result<(), services::keys::Error>),
    AllKeysRedeemed,
    BackupDescriptor,
    ExportEncryptedDescriptor(Result<Box<LianaDescriptor>, encrypted_backup::Error>),
    ExportXpub(String),
    ImportExport(ImportExportMessage),
    ImportBackup,
    WalletFromBackup((HashMap<Fingerprint, settings::KeySetting>, Backup)),
    WalletAliasEdited(String),
    SelectAccount(Fingerprint, ChildNumber),
    OpenUrl(String),
    SelectKeySource(SelectKeySourceMessage),
    EditKeyAlias(EditKeyAliasMessage),
    Decrypt(Decrypt),
    None,
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
    ExistingConnectAccounts(Vec<String>),
    SelectConnectAccount(String),
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
    ImportDescriptorFromFile,
    ImportExport(ImportExportMessage),
}

#[derive(Debug, Clone)]
pub enum DefineBitcoind {
    ConfigFieldEdited(ConfigField, String),
    RpcAuthTypeSelected(RpcAuthType),
}

#[derive(Debug, Clone)]
pub enum DefineElectrum {
    ConfigFieldEdited(electrum::ConfigField, String),
    ValidDomainChanged(bool),
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
    // NOTE: KeysEdit & KeysEdited takes a Vec<coordinate>
    // in order to assign a key to several path from a single
    // modal call
    KeysEdited(Vec<(usize, usize)>, SelectedKey),
    KeysEdit(PathKind, Vec<(usize, usize)>),
    Path(usize, DefinePath),
    AddRecoveryPath,
    AddSafetyNetPath,
    ThresholdSequenceModal(ThresholdSequenceModal),
    Reset,
    AliasEdited(Fingerprint, String /* alias*/),
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
    EditAlias,
    Clipboard(String),
}

#[derive(Debug, Clone)]
pub enum ThresholdSequenceModal {
    ThresholdEdited(usize),
    SequenceEdited(String),
    Confirm,
}
