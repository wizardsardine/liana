use coincube_core::{
    descriptors::CoincubeDescriptor,
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
        settings::{self, CubeSettings, ProviderKey},
        view::Close,
    },
    backup::Backup,
    export::ImportExportMessage,
    hw::HardwareWalletMessage,
    installer::{decrypt::Decrypt, descriptor::PathKind},
    node::{
        bitcoind::{Bitcoind, ConfigField, RpcAuthType},
        electrum, esplora, NodeType,
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
    BackToApp(Network),
    Install,
    Close,
    Reload,
    Select(usize),
    UseMasterSigner,
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
    ExportEncryptedDescriptor(Result<Box<CoincubeDescriptor>, encrypted_backup::Error>),
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
    CubeSaved(
        Result<CubeSettings, String>,
        Box<settings::WalletSettings>,
        Option<Bitcoind>,
    ),
    CubeSaveFailed(String),
    RetryCubeSave,
    /// Result of the post-install `create_connect_vault` orchestration
    /// (see `installer/connect_vault.rs`). Emitted after
    /// `Installed(Ok)` so the Final step can surface the outcome.
    ConnectVaultCreated(
        Result<super::connect_vault::ConnectVaultOutcome, super::connect_vault::ConnectVaultError>,
    ),
    CoincubeConnect(CoincubeConnectMsg),
    /// Cube Recovery Kit restore step (W13 / W14 / W15).
    RecoveryKitRestore(RecoveryKitRestoreMsg),
    BorderWalletWizard(
        super::step::descriptor::editor::border_wallet_wizard::BorderWalletWizardMessage,
    ),
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
pub enum DefineEsplora {
    ConfigFieldEdited(esplora::ConfigField, String),
}

#[derive(Debug, Clone)]
pub enum DefineNode {
    NodeTypeSelected(NodeType),
    DefineBitcoind(DefineBitcoind),
    DefineElectrum(DefineElectrum),
    DefineEsplora(DefineEsplora),
    PingResult((NodeType, Result<(), Error>)),
    Ping,
}

#[derive(Debug, Clone)]
pub enum SelectBitcoindTypeMsg {
    // Default card
    ContinueWithConnect,
    ToggleInstallNode,
    ToggleAdvanced,
    // Advanced options
    UseExternal(bool),
    UseConnect,
}

#[derive(Debug, Clone)]
pub enum CoincubeConnectMsg {
    EmailEdited(String),
    ToggleMode,
    RequestOtp,
    OtpRequested(Result<(), String>),
    OtpEdited(String),
    OtpVerified(Result<String, String>),
    ResendOtp,
    OtpResent(Result<(), String>),
    Skip,
}

/// Messages driving the Cube Recovery Kit restore step. Manual
/// `Debug` redacts every variant that carries password, OTP, or
/// mnemonic material — tracing dumps don't leak the kit.
#[derive(Clone)]
pub enum RecoveryKitRestoreMsg {
    EmailEdited(String),
    RequestOtp,
    OtpSent(Result<(), String>),
    OtpEdited(String),
    OtpVerified(Result<String, String>),
    CubesLoaded(Result<Vec<super::step::recovery_kit_restore::RestoreCubeCandidate>, String>),
    SelectCube(u64),
    PasswordEdited(String),
    SubmitPassword,
    DecryptResult(
        Result<
            (
                Option<crate::services::recovery::SeedBlob>,
                Option<crate::services::recovery::DescriptorBlob>,
            ),
            String,
        >,
    ),
    RetryFromStart,
    Skip,
}

impl std::fmt::Debug for RecoveryKitRestoreMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmailEdited(_) => f.debug_tuple("EmailEdited").field(&"<redacted>").finish(),
            Self::RequestOtp => write!(f, "RequestOtp"),
            Self::OtpSent(r) => f.debug_tuple("OtpSent").field(r).finish(),
            Self::OtpEdited(_) => f.debug_tuple("OtpEdited").field(&"<redacted>").finish(),
            Self::OtpVerified(Ok(_)) => write!(f, "OtpVerified(Ok(<redacted>))"),
            Self::OtpVerified(Err(e)) => f
                .debug_tuple("OtpVerified")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::CubesLoaded(r) => f.debug_tuple("CubesLoaded").field(r).finish(),
            Self::SelectCube(id) => f.debug_tuple("SelectCube").field(id).finish(),
            Self::PasswordEdited(_) => f
                .debug_tuple("PasswordEdited")
                .field(&"<redacted>")
                .finish(),
            Self::SubmitPassword => write!(f, "SubmitPassword"),
            Self::DecryptResult(Ok(_)) => write!(f, "DecryptResult(Ok(<redacted>))"),
            Self::DecryptResult(Err(e)) => f
                .debug_tuple("DecryptResult")
                .field(&Err::<(), _>(e))
                .finish(),
            Self::RetryFromStart => write!(f, "RetryFromStart"),
            Self::Skip => write!(f, "Skip"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum InternalBitcoindMsg {
    Previous,
    Reload,
    DefineConfig,
    Download,
    DownloadProgressed(super::step::DownloadUpdate),
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
    /// Open the Border Wallet wizard for the given path coordinates.
    OpenBorderWalletWizard(Vec<(usize, usize)>),
    /// Close the current modal and re-open the Select-key-source picker
    /// for the given coordinates. Used by the Border Wallet wizard's
    /// intro "Back" button to return to the picker instead of dropping
    /// the user back to the descriptor editor.
    ReopenKeyModal(Vec<(usize, usize)>),
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
