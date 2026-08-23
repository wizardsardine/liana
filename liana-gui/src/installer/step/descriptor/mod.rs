pub mod editor;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
};

use iced::{Subscription, Task};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use liana_ui::{
    component::form,
    widget::{modal::Modal, Element},
};

use async_hwi::DeviceKind;

use crate::{
    airgap::{
        self,
        device::AirgappedSignerConfig,
        exchange::{Answer, Ask},
    },
    app::{
        settings::KeySetting,
        state::{
            airgap::{AirgapAction, AirgapModal, AirgapOutcome},
            export::ExportModal,
        },
        wallet::wallet_name,
    },
    backup::Backup,
    dir::LianaDirectory,
    export::{ImportExportMessage, ImportExportType, Progress},
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        decrypt::{Decrypt, DecryptModal},
        message::{self, Message},
        step::import_descriptor::ImportDescriptorModal,
        step::{Context, Step},
        view, Error,
    },
};

pub struct ImportDescriptor {
    network: Network,
    wrong_network: bool,
    error: Option<String>,
    modal: ImportDescriptorModal,
    imported_descriptor: form::Value<String>,
    imported_backup: Option<Backup>,
    imported_aliases: Option<HashMap<Fingerprint, KeySetting>>,
    paste_descriptor_expanded: bool,
}

impl ImportDescriptor {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            imported_descriptor: form::Value::default(),
            wrong_network: false,
            error: None,
            modal: ImportDescriptorModal::None,
            imported_backup: None,
            imported_aliases: None,
            paste_descriptor_expanded: false,
        }
    }

    fn check_descriptor(&mut self, network: Network) -> Option<LianaDescriptor> {
        if !self.imported_descriptor.value.is_empty() {
            if let Ok(desc) = LianaDescriptor::from_str(&self.imported_descriptor.value) {
                if network == Network::Bitcoin {
                    self.imported_descriptor.valid = desc.all_xpubs_net_is(network);
                } else {
                    self.imported_descriptor.valid = desc.all_xpubs_net_is(Network::Testnet);
                }
                if self.imported_descriptor.valid {
                    self.wrong_network = false;
                    Some(desc)
                } else {
                    self.wrong_network = true;
                    None
                }
            } else {
                self.imported_descriptor.valid = false;
                self.wrong_network = false;
                None
            }
        } else {
            self.wrong_network = false;
            self.imported_descriptor.valid = true;
            None
        }
    }
}

impl Step for ImportDescriptor {
    // ImportRemoteWallet is used instead
    fn skip(&self, ctx: &Context) -> bool {
        ctx.remote_backend.is_some()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        self.modal.subscriptions(hws)
    }

    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        let task = match message {
            Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(desc)) => {
                // If user manually change the descriptor, then the imported backup
                // becomes invalid;
                if desc != self.imported_descriptor.value {
                    self.imported_backup = None;
                    self.imported_aliases = None;
                }
                self.imported_descriptor.value = desc;
                self.check_descriptor(self.network);
                None
            }
            Message::DefineDescriptor(message::DefineDescriptor::ShowImportDescriptor(
                expanded,
            )) => {
                self.paste_descriptor_expanded = expanded;
                None
            }
            Message::ImportBackup => {
                self.imported_backup = None;
                let modal = ExportModal::new(None, ImportExportType::FromBackup(self.network));
                let launch = modal.launch(false);
                self.modal = ImportDescriptorModal::Export(modal);
                Some(launch)
            }
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = ImportDescriptorModal::None;
                None
            }
            Message::ImportExport(ImportExportMessage::Progress(Progress::WalletFromBackup(r))) => {
                let (descriptor, network, aliases, backup) = r;
                if let Some(n) = network {
                    if self.network == n {
                        self.imported_backup = Some(backup);
                        self.imported_descriptor.value = descriptor.to_string();
                        self.imported_aliases = Some(aliases);
                    } else {
                        self.error = Some(crate::t!("installer-backup-network-mismatch"));
                    }
                } else {
                    // The backup have been inferred from a bare descriptor, we check whether
                    // the descriptor match any test network
                    if self.network != Network::Bitcoin {
                        self.imported_backup = Some(backup);
                        self.imported_descriptor.value = descriptor.to_string();
                        self.imported_aliases = Some(aliases);
                    } else {
                        self.error = Some(crate::t!("installer-backup-network-mismatch"));
                    }
                }
                None
            }
            Message::ImportExport(ImportExportMessage::Progress(Progress::EncryptedFile(
                bytes,
            ))) => {
                self.modal = ImportDescriptorModal::Decrypt(DecryptModal::new(bytes, self.network));
                None
            }
            Message::ImportExport(m) => Some(self.modal.update(Message::ImportExport(m))),
            Message::HardwareWalletUpdate => {
                if let ImportDescriptorModal::Decrypt(modal) = &mut self.modal {
                    modal.update_devices(hws)
                } else {
                    None
                }
            }
            Message::Decrypt(Decrypt::Close) => {
                if matches!(self.modal, ImportDescriptorModal::Decrypt(_)) {
                    self.modal = ImportDescriptorModal::None;
                }
                None
            }
            Message::Decrypt(Decrypt::Backup(mut backup))
                if matches!(self.modal, ImportDescriptorModal::Decrypt(_)) =>
            {
                let descriptor = backup.accounts.first().map(|acc| acc.descriptor.clone());
                if let Some(desc) = descriptor {
                    let network_matches = if self.network == Network::Bitcoin {
                        backup.network == Network::Bitcoin
                    } else {
                        backup.network != Network::Bitcoin
                    };
                    if network_matches {
                        // NOTE: we need to overwrite w/ correct network for testnets
                        // as non Mainnet keys / descriptor are parsed as Signet
                        backup.network = self.network;

                        self.imported_descriptor.value = desc;
                        self.imported_backup = Some(backup);
                        self.imported_aliases = None;
                        self.modal = ImportDescriptorModal::None;
                    } else {
                        self.modal = ImportDescriptorModal::None;
                        self.error = Some(crate::t!("installer-backup-network-mismatch"));
                    }
                } else {
                    self.modal = ImportDescriptorModal::None;
                    self.error = Some(crate::t!("installer-backup-descriptor-missing"));
                }
                None
            }
            Message::Decrypt(msg) => Some(self.modal.update(Message::Decrypt(msg))),
            _ => None,
        };
        task.unwrap_or(Task::none())
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // Set to true in order to force the registration process to be shown to user.
        ctx.hw_is_used = true;
        // descriptor forms for import or creation cannot be both empty or filled.
        if let Some(desc) = self.check_descriptor(self.network) {
            ctx.descriptor = Some(desc.clone());
            ctx.backup = Some(match &self.imported_backup {
                Some(backup) => backup.clone(),
                None => Backup::from_descriptor(desc, self.network),
            });
        } else {
            return false;
        }

        if let Some(aliases) = &self.imported_aliases {
            ctx.keys = aliases.clone();
        }

        if let Some(wallet_alias) = self.imported_backup.as_ref().and_then(|b| b.alias.clone()) {
            ctx.wallet_alias = wallet_alias;
        }
        true
    }

    fn revert(&self, ctx: &mut Context) {
        ctx.keys = HashMap::new();
        ctx.backup = None;
        ctx.descriptor = None;
        ctx.wallet_alias = String::new();
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        network: Network,
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let content = view::import_descriptor(
            progress,
            network,
            email,
            &self.imported_descriptor,
            self.imported_backup.is_some(),
            self.wrong_network,
            self.error.as_ref(),
            self.paste_descriptor_expanded,
        );
        self.modal.view(content)
    }
}

impl From<ImportDescriptor> for Box<dyn Step> {
    fn from(s: ImportDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct RegisterDescriptor {
    descriptor: Option<LianaDescriptor>,
    processing: bool,
    chosen_hw: Option<usize>,
    hmacs: Vec<(Fingerprint, DeviceKind, Option<[u8; 32]>)>,
    registered: HashSet<Fingerprint>,
    airgapped_signers: Vec<AirgappedSignerConfig>,
    /// The air-gapped signer the open exchange registers with.
    registering: Option<Fingerprint>,
    airgap: Option<AirgapModal>,
    /// Where the air-gap display settings are kept, set from the context.
    liana_directory: LianaDirectory,
    error: Option<Error>,
    done: bool,
    /// Whether this step is part of the descriptor creation process. This is used to detect when
    /// it's instead shown as part of the descriptor *import* process, where we can't detect
    /// whether a signing device is used, to explicit this step is not required if the user isn't
    /// using a signing device.
    created_desc: bool,
}

impl RegisterDescriptor {
    fn new(created_desc: bool) -> Self {
        Self {
            created_desc,
            descriptor: Default::default(),
            processing: Default::default(),
            chosen_hw: Default::default(),
            hmacs: Default::default(),
            registered: Default::default(),
            airgapped_signers: Default::default(),
            registering: None,
            airgap: None,
            liana_directory: LianaDirectory::new(PathBuf::new()),
            error: Default::default(),
            done: Default::default(),
        }
    }

    pub fn new_create_wallet() -> Self {
        Self::new(true)
    }

    pub fn new_import_wallet() -> Self {
        Self::new(false)
    }
}

impl Step for RegisterDescriptor {
    fn load_context(&mut self, ctx: &Context) {
        // we reset device registered set if the descriptor have changed.
        if self.descriptor != ctx.descriptor {
            self.registered = Default::default();
            self.done = false;
        }
        self.descriptor.clone_from(&ctx.descriptor);
        self.airgapped_signers = ctx.airgapped_signers.values().cloned().collect();
        self.liana_directory = ctx.liana_directory.clone();
        let mut map = HashMap::new();
        for key in ctx.keys.values().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
    }
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::SelectAirgappedSigner(fingerprint) => {
                let (Some(descriptor), Some(signer)) = (
                    self.descriptor.as_ref(),
                    self.airgapped_signers
                        .iter()
                        .find(|signer| signer.fingerprint == fingerprint),
                ) else {
                    return Task::none();
                };
                match AirgapModal::new(
                    crate::t!("airgap-register-title"),
                    Ask::Register {
                        wallet: airgap::exchange::Wallet {
                            // the very alias the USB path below registers under
                            alias: wallet_name(descriptor),
                            descriptor: descriptor.clone(),
                            registration: signer.registration.clone(),
                            capabilities: signer.capabilities,
                        },
                    },
                    &self.liana_directory,
                ) {
                    Ok(modal) => {
                        self.error = None;
                        self.registering = Some(fingerprint);
                        self.airgap = Some(modal);
                    }
                    Err(e) => self.error = Some(e.into()),
                }
            }
            Message::Airgap(action) => {
                let Some(airgap) = self.airgap.as_mut() else {
                    return Task::none();
                };
                let task = airgap.update(action).map(Message::Airgap);
                return match airgap.take_outcome() {
                    Some(AirgapOutcome::Answer(answer)) => {
                        self.airgap = None;
                        let (Some(fingerprint), Answer::Registered(registration)) =
                            (self.registering.take(), *answer)
                        else {
                            return Task::none();
                        };
                        if let Some(signer) = self
                            .airgapped_signers
                            .iter_mut()
                            .find(|signer| signer.fingerprint == fingerprint)
                        {
                            signer.registration = registration;
                            signer.registration.descriptor_checksum =
                                self.descriptor.as_ref().map(LianaDescriptor::checksum);
                            self.registered.insert(fingerprint);
                        }
                        Task::none()
                    }
                    Some(AirgapOutcome::Cancelled) => {
                        self.airgap = None;
                        self.registering = None;
                        Task::none()
                    }
                    None => task,
                };
            }
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    ..
                }) = hws.list.get(i)
                {
                    if !self.registered.contains(fingerprint) {
                        let descriptor = self.descriptor.as_ref().unwrap();
                        let name = wallet_name(descriptor);
                        self.chosen_hw = Some(i);
                        self.processing = true;
                        self.error = None;
                        return Task::perform(
                            register_wallet(
                                device.clone(),
                                *fingerprint,
                                name,
                                descriptor.to_string(),
                            ),
                            Message::WalletRegistered,
                        );
                    }
                }
            }
            Message::WalletRegistered(res) => {
                self.processing = false;
                self.chosen_hw = None;
                match res {
                    Ok((fingerprint, hmac)) => {
                        if let Some(hw_h) = hws
                            .list
                            .iter()
                            .find(|hw_h| hw_h.fingerprint() == Some(fingerprint))
                        {
                            self.registered.insert(fingerprint);
                            self.hmacs.push((fingerprint, *hw_h.kind(), hmac));
                        }
                    }
                    Err(e) => {
                        if !matches!(e, Error::HardwareWallet(async_hwi::Error::UserRefused)) {
                            self.error = Some(e)
                        }
                    }
                }
            }
            Message::Reload => {
                return self.load();
            }
            Message::UserActionDone(done) => {
                self.done = done;
            }
            _ => {}
        };
        Task::none()
    }
    fn skip(&self, ctx: &Context) -> bool {
        !ctx.hw_is_used && ctx.airgapped_signers.is_empty()
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        for (fingerprint, kind, token) in &self.hmacs {
            ctx.hws.push((*kind, *fingerprint, *token));
        }
        for signer in &self.airgapped_signers {
            ctx.airgapped_signers
                .insert(signer.fingerprint, signer.clone());
        }
        true
    }
    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        let hw = hws.refresh().map(Message::HardwareWallets);
        match &self.airgap {
            Some(airgap) => Subscription::batch([hw, airgap.subscription().map(Message::Airgap)]),
            None => hw,
        }
    }
    fn load(&self) -> Task<Message> {
        Task::none()
    }
    fn view<'a>(
        &'a self,
        hws: &'a HardwareWallets,
        progress: (usize, usize),
        network: Network,
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let desc = self.descriptor.as_ref().unwrap();

        let content = view::register_descriptor(
            progress,
            network,
            email,
            desc,
            &hws.list,
            &self.airgapped_signers,
            &self.registered,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
            self.done,
            self.created_desc,
        );
        match &self.airgap {
            Some(airgap) => Modal::new(content, airgap.view(Message::Airgap))
                .on_blur(Some(Message::Airgap(AirgapAction::Cancel)))
                .into(),
            None => content,
        }
    }
}

async fn register_wallet(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    name: String,
    descriptor: String,
) -> Result<(Fingerprint, Option<[u8; 32]>), Error> {
    let hmac = hw
        .register_wallet(&name, &descriptor)
        .await
        .map_err(Error::from)?;
    Ok((fingerprint, hmac))
}

impl From<RegisterDescriptor> for Box<dyn Step> {
    fn from(s: RegisterDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct BackupDescriptor {
    done: bool,
    descriptor: Option<LianaDescriptor>,
    keys: HashMap<Fingerprint, KeySetting>,
    modal: Option<ExportModal>,
    error: Option<Error>,
    context: Option<Context>,
    help_open: bool,
    privacy_mode: bool,
}

impl Step for BackupDescriptor {
    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        if let Some(modal) = &self.modal {
            if let Some(sub) = modal.subscription() {
                sub.map(|m| Message::ImportExport(ImportExportMessage::Progress(m)))
            } else {
                Subscription::none()
            }
        } else {
            Subscription::none()
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = None;
            }
            Message::ImportExport(m) => {
                if let Some(modal) = self.modal.as_mut() {
                    let task: Task<Message> = modal.update(m);
                    return task;
                };
            }
            Message::BackupDescriptor => {
                if let (None, Some(ctx)) = (&self.modal, self.context.as_ref()) {
                    let descriptor = ctx.descriptor.clone();
                    return Task::perform(
                        async move {
                            let descriptor = descriptor.ok_or(bip138::Error::String(Box::new(
                                crate::t!("installer-descriptor-missing"),
                            )))?;
                            Ok(Box::new(descriptor))
                        },
                        Message::ExportEncryptedDescriptor,
                    );
                }
            }
            Message::ExportEncryptedDescriptor(bytes) => {
                if self.modal.is_none() {
                    let bytes = match bytes {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!("{e:?}");
                            self.error = Some(Error::Backup(e));
                            return Task::none();
                        }
                    };
                    let modal = ExportModal::new(
                        None,
                        ImportExportType::ExportEncryptedDescriptor {
                            descriptor: bytes,
                            store_common_paths: !self.privacy_mode,
                        },
                    );
                    let launch = modal.launch(true);
                    self.modal = Some(modal);
                    return launch;
                }
            }
            Message::UserActionDone(done) => {
                self.done = done;
            }
            Message::ShowBackupDescriptorHelp(open) => {
                self.help_open = open;
            }
            Message::BackupPrivacyMode(privacy_mode) => {
                self.privacy_mode = privacy_mode;
            }
            _ => {}
        }
        Task::none()
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.encrypted_backup_privacy_mode = self.privacy_mode;
        true
    }
    fn load_context(&mut self, ctx: &Context) {
        self.context = Some(ctx.clone());
        if self.descriptor != ctx.descriptor {
            self.descriptor.clone_from(&ctx.descriptor);
            self.done = false;
        }
        self.keys = ctx
            .keys
            .values()
            .map(|k| (k.master_fingerprint, k.clone()))
            .collect();
    }
    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        network: Network,
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let content = view::backup_descriptor(
            progress,
            network,
            email,
            self.descriptor.as_ref().expect("Must be a descriptor"),
            &self.keys,
            self.error.as_ref(),
            self.done,
            self.help_open,
            self.privacy_mode,
        );
        if let Some(modal) = &self.modal {
            modal.view(content)
        } else {
            content
        }
    }
}

impl From<BackupDescriptor> for Box<dyn Step> {
    fn from(s: BackupDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}
