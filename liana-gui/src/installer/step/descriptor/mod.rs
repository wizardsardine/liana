pub mod editor;

use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use iced::{Subscription, Task};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use liana_ui::{component::form, widget::Element};

use async_hwi::DeviceKind;

use crate::{
    app::{settings::KeySetting, state::export::ExportModal, wallet::wallet_name},
    backup::{self, Backup},
    export::{ImportExportMessage, ImportExportType, Progress},
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view, Error,
    },
};

pub struct ImportDescriptor {
    network: Network,
    wrong_network: bool,
    error: Option<String>,
    modal: Option<ExportModal>,
    imported_descriptor: form::Value<String>,
    imported_backup: Option<Backup>,
    imported_aliases: Option<HashMap<Fingerprint, KeySetting>>,
}

impl ImportDescriptor {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            imported_descriptor: form::Value::default(),
            wrong_network: false,
            error: None,
            modal: None,
            imported_backup: None,
            imported_aliases: None,
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
            Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(desc)) => {
                // If user manually change the descriptor, then the imported backup
                // becomes invalid;
                if desc != self.imported_descriptor.value {
                    self.imported_backup = None;
                    self.imported_aliases = None;
                }
                self.imported_descriptor.value = desc;
                self.check_descriptor(self.network);
            }
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = None;
            }
            Message::ImportBackup => {
                self.imported_backup = None;
                let modal = ExportModal::new(None, ImportExportType::WalletFromBackup);
                let launch = modal.launch(false);
                self.modal = Some(modal);
                return launch;
            }
            Message::ImportExport(ImportExportMessage::Progress(Progress::WalletFromBackup(r))) => {
                let (descriptor, network, aliases, backup) = r;
                if self.network == network {
                    self.imported_backup = Some(backup);
                    self.imported_descriptor.value = descriptor.to_string();
                    self.imported_aliases = Some(aliases);
                } else {
                    self.error = Some("Backup network do not match the selected network!".into());
                }
            }
            Message::ImportExport(m) => {
                if let Some(modal) = self.modal.as_mut() {
                    let task: Task<Message> = modal.update(m);
                    return task;
                };
            }
            _ => {}
        }
        Task::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // Set to true in order to force the registration process to be shown to user.
        ctx.hw_is_used = true;
        // descriptor forms for import or creation cannot be both empty or filled.
        if let Some(desc) = self.check_descriptor(self.network) {
            ctx.descriptor = Some(desc);
        } else {
            return false;
        }

        if let Some(backup) = &self.imported_backup {
            ctx.backup = Some(backup.clone());
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
        email: Option<&'a str>,
    ) -> Element<Message> {
        let content = view::import_descriptor(
            progress,
            email,
            &self.imported_descriptor,
            self.imported_backup.is_some(),
            self.wrong_network,
            self.error.as_ref(),
        );
        if let Some(modal) = &self.modal {
            modal.view(content)
        } else {
            content
        }
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
        let mut map = HashMap::new();
        for key in ctx.keys.values().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
    }
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
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
        !ctx.hw_is_used
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        for (fingerprint, kind, token) in &self.hmacs {
            ctx.hws.push((*kind, *fingerprint, *token));
        }
        true
    }
    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        hws.refresh().map(Message::HardwareWallets)
    }
    fn load(&self) -> Task<Message> {
        Task::none()
    }
    fn view<'a>(
        &'a self,
        hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let desc = self.descriptor.as_ref().unwrap();

        view::register_descriptor(
            progress,
            email,
            desc,
            &hws.list,
            &self.registered,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
            self.done,
            self.created_desc,
        )
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
            Message::BackupWallet => {
                if let (None, Some(ctx)) = (&self.modal, self.context.as_ref()) {
                    let ctx = ctx.clone();
                    return Task::perform(
                        async move {
                            let backup = Backup::from_installer_descriptor_step(ctx).await?;
                            serde_json::to_string_pretty(&backup).map_err(|_| backup::Error::Json)
                        },
                        Message::ExportWallet,
                    );
                }
            }
            Message::ExportWallet(str) => {
                if self.modal.is_none() {
                    let str = match str {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("{e:?}");
                            self.error = Some(Error::Backup(e));
                            return Task::none();
                        }
                    };
                    let modal = ExportModal::new(None, ImportExportType::ExportBackup(str));
                    let launch = modal.launch(true);
                    self.modal = Some(modal);
                    return launch;
                }
            }
            Message::UserActionDone(done) => {
                self.done = done;
            }
            _ => {}
        }
        Task::none()
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
        email: Option<&'a str>,
    ) -> Element<Message> {
        let content = view::backup_descriptor(
            progress,
            email,
            self.descriptor.as_ref().expect("Must be a descriptor"),
            &self.keys,
            self.error.as_ref(),
            self.done,
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
