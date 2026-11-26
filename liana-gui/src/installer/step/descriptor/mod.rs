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
    component::{form, panels::home::WalletOrigin},
    widget::{modal::Modal, Element},
};

use async_hwi::DeviceKind;

use crate::{
    airgap::device::{AirgappedDevice, Answer, Registration, RequestBuilder},
    app::{
        settings::KeySetting,
        state::{
            airgap::{AirgapModal, AirgapOutcome, Message as AirgapMessage},
            export::ExportModal,
        },
        wallet::wallet_name,
    },
    backup::bip139::{Bip139Backup, Bip139Network},
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
    imported_backup: Option<Bip139Backup>,
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
                        self.imported_backup = backup;
                        self.imported_descriptor.value = descriptor.to_string();
                        self.imported_aliases = Some(aliases);
                    } else {
                        self.error = Some(crate::t!("installer-backup-network-mismatch"));
                    }
                } else {
                    // The backup have been inferred from a bare descriptor, we check whether
                    // the descriptor match any test network
                    if self.network != Network::Bitcoin {
                        self.imported_backup = backup;
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
                let descriptor = backup
                    .accounts
                    .first()
                    .and_then(|acc| acc.liana_descriptor())
                    .map(|descriptor| descriptor.to_string());
                if let Some(desc) = descriptor {
                    let network_matches = if self.network == Network::Bitcoin {
                        backup.network == Bip139Network::Bitcoin
                    } else {
                        backup.network != Bip139Network::Bitcoin
                    };
                    if network_matches {
                        // NOTE: we need to overwrite w/ correct network for testnets
                        // as non Mainnet keys / descriptor are parsed as Signet
                        backup.network = self.network.into();

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
            Message::Decrypt(Decrypt::Descriptor(descriptor))
                if matches!(self.modal, ImportDescriptorModal::Decrypt(_)) =>
            {
                self.modal = ImportDescriptorModal::None;
                if descriptor.all_xpubs_net_is(Network::Bitcoin)
                    == (self.network == Network::Bitcoin)
                {
                    self.imported_descriptor.value = descriptor.to_string();
                    self.imported_backup = None;
                    self.imported_aliases = None;
                } else {
                    self.error = Some(crate::t!("installer-backup-network-mismatch"));
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
            ctx.backup = self.imported_backup.clone();
            ctx.wallet_origin = Some(if self.imported_backup.is_some() {
                WalletOrigin::Backup
            } else {
                WalletOrigin::Descriptor
            });
        } else {
            return false;
        }

        if let Some(aliases) = &self.imported_aliases {
            ctx.keys = aliases.clone();
            ctx.provider_keys = HashMap::new();
        }

        if let (Some(backup), Some(descriptor)) = (&self.imported_backup, &ctx.descriptor) {
            if let Some(name) = backup
                .name
                .clone()
                .filter(|name| *name != wallet_name(descriptor))
            {
                ctx.wallet_alias = name;
            }
        }
        true
    }

    fn revert(&self, ctx: &mut Context) {
        ctx.keys = HashMap::new();
        ctx.provider_keys = HashMap::new();
        ctx.backup = None;
        ctx.wallet_origin = None;
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
    airgapped_signers: Vec<AirgappedDevice>,
    modal: Option<AirgapModal>,
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
            modal: None,
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

    fn on_select_airgapped_signer(&mut self, fingerprint: Fingerprint) -> Task<Message> {
        let (Some(descriptor), Some(signer)) = (
            self.descriptor.as_ref(),
            self.airgapped_signers
                .iter()
                .find(|signer| signer.fingerprint == fingerprint),
        ) else {
            log::warn!("no descriptor or no signing device {fingerprint} to register with");
            return Task::none();
        };
        let builder = RequestBuilder {
            alias: wallet_name(descriptor),
            descriptor: descriptor.clone(),
            registration: signer
                .registration(&descriptor.checksum())
                .cloned()
                .unwrap_or_default(),
            capabilities: signer.capabilities,
        };
        match builder.register().and_then(|request| {
            AirgapModal::new(
                Some(fingerprint),
                crate::t!("airgap-register-title"),
                request,
                &self.liana_directory,
            )
        }) {
            Ok((modal, task)) => {
                self.error = None;
                self.modal = Some(modal);
                task.map(Message::Airgap)
            }
            Err(e) => {
                self.error = Some(e.into());
                Task::none()
            }
        }
    }

    fn on_airgap(&mut self, action: AirgapMessage) -> Task<Message> {
        let Some(modal) = self.modal.as_mut() else {
            return Task::none();
        };
        let task = modal.update(action).map(Message::Airgap);
        match modal.outcome.take() {
            Some(AirgapOutcome::Answer(answer)) => {
                let fingerprint = modal.fingerprint;
                self.modal = None;
                let (Some(fingerprint), Answer::Registered(registration)) = (fingerprint, *answer)
                else {
                    return task;
                };
                if let Some(signer) = self
                    .airgapped_signers
                    .iter_mut()
                    .find(|signer| signer.fingerprint == fingerprint)
                {
                    signer.record(Registration {
                        descriptor_checksum: self
                            .descriptor
                            .as_ref()
                            .map(LianaDescriptor::checksum),
                        ..registration
                    });
                    self.registered.insert(fingerprint);
                }
                task
            }
            Some(AirgapOutcome::Cancelled) => {
                self.modal = None;
                task
            }
            None => task,
        }
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
                return self.on_select_airgapped_signer(fingerprint);
            }
            Message::Airgap(action) => return self.on_airgap(action),
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
        match &self.modal {
            Some(modal) => Subscription::batch([hw, modal.subscription().map(Message::Airgap)]),
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
        match &self.modal {
            Some(modal) => Modal::new(content, crate::view::airgap::modal(modal, Message::Airgap))
                .on_blur(Some(Message::Airgap(AirgapMessage::Cancel)))
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
