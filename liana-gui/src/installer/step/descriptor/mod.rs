pub mod editor;

use std::{
    collections::{HashMap, HashSet},
    fs,
    str::FromStr,
};

use iced::{widget::qr_code, Subscription, Task};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use liana_ui::{component::form, widget::Element};

use async_hwi::DeviceKind;

use crate::{
    airgap::{
        encode_ur, AirgappedRequest, AirgappedSignerConfig, AnimatedQr, PolicyRegistration,
        QrDensity, UrPayload,
    },
    app::{settings::KeySetting, state::export::ExportModal, wallet::wallet_name},
    backup::Backup,
    export::{get_path, ImportExportMessage, ImportExportType, Progress},
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        decrypt::{Decrypt, DecryptModal},
        message::{self, Message},
        step::import_descriptor::{ImportDescriptorModal, BACKUP_NETWORK_NOT_MATCH},
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
                let modal = ExportModal::new(None, ImportExportType::FromBackup);
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
                        self.error = Some(BACKUP_NETWORK_NOT_MATCH.into());
                    }
                } else {
                    // The backup have been inferred from a bare descriptor, we check whether
                    // the descriptor match any test network
                    if self.network != Network::Bitcoin {
                        self.imported_backup = Some(backup);
                        self.imported_descriptor.value = descriptor.to_string();
                        self.imported_aliases = Some(aliases);
                    } else {
                        self.error = Some(BACKUP_NETWORK_NOT_MATCH.into());
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
            Message::Decrypt(Decrypt::Backup(mut backup)) => {
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
                        self.error = Some(BACKUP_NETWORK_NOT_MATCH.into());
                    }
                } else {
                    self.modal = ImportDescriptorModal::None;
                    self.error = Some("Backup imported but descriptor missing!".into());
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
        ctx.airgapped_signers.clear();
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
    error: Option<Error>,
    done: bool,
    /// Whether this step is part of the descriptor creation process. This is used to detect when
    /// it's instead shown as part of the descriptor *import* process, where we can't detect
    /// whether a signing device is used, to explicit this step is not required if the user isn't
    /// using a signing device.
    created_desc: bool,
    network: Network,
    airgapped_signers: Vec<AirgappedSignerConfig>,
    passport_qr: Option<PassportRegistrationQr>,
}

struct PassportRegistrationQr {
    fingerprint: Fingerprint,
    payload: UrPayload,
    density: QrDensity,
    animation: AnimatedQr,
    qr_data: qr_code::Data,
}

impl PassportRegistrationQr {
    fn new(fingerprint: Fingerprint, payload: UrPayload) -> Result<Self, String> {
        let density = QrDensity::default();
        let (animation, qr_data) = Self::encode(&payload, density)?;
        Ok(Self {
            fingerprint,
            payload,
            density,
            animation,
            qr_data,
        })
    }

    fn encode(
        payload: &UrPayload,
        density: QrDensity,
    ) -> Result<(AnimatedQr, qr_code::Data), String> {
        let animation = encode_ur(payload, density.fragment_length())
            .and_then(|encoded| AnimatedQr::new(encoded, 5))
            .map_err(|error| error.to_string())?;
        let frame = animation
            .frame()
            .ok_or_else(|| "QR animation has no frames".to_owned())?;
        let qr_data = qr_code::Data::new(frame).map_err(|error| error.to_string())?;
        Ok((animation, qr_data))
    }

    fn set_density(&mut self, density: QrDensity) -> Result<(), String> {
        let (animation, qr_data) = Self::encode(&self.payload, density)?;
        self.density = density;
        self.animation = animation;
        self.qr_data = qr_data;
        Ok(())
    }

    fn refresh(&mut self) {
        if let Some(frame) = self.animation.frame() {
            if let Ok(data) = qr_code::Data::new(frame) {
                self.qr_data = data;
            }
        }
    }
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
            network: Network::Bitcoin,
            airgapped_signers: Vec::new(),
            passport_qr: None,
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
        self.network = ctx.network;
        self.airgapped_signers = ctx.airgapped_signers.values().cloned().collect();
        self.airgapped_signers
            .sort_by_key(|signer| signer.fingerprint);
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
            Message::RegisterPassport(fingerprint) => {
                let Some(descriptor) = self.descriptor.as_ref() else {
                    return Task::none();
                };
                let registration = PolicyRegistration::from_descriptor(
                    wallet_name(descriptor),
                    self.network,
                    descriptor,
                );
                match registration
                    .and_then(|registration| {
                        AirgappedRequest::RegisterPolicy(registration).encode()
                    })
                    .map_err(|error| error.to_string())
                    .and_then(|payload| PassportRegistrationQr::new(fingerprint, payload))
                {
                    Ok(qr) => {
                        self.passport_qr = Some(qr);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(Error::Unexpected(error)),
                }
            }
            Message::PassportQrTick => {
                if let Some(qr) = &mut self.passport_qr {
                    qr.refresh();
                }
            }
            Message::PausePassportQr => {
                if let Some(qr) = &mut self.passport_qr {
                    qr.animation.pause();
                }
            }
            Message::ResumePassportQr => {
                if let Some(qr) = &mut self.passport_qr {
                    qr.animation.resume();
                }
            }
            Message::RestartPassportQr => {
                if let Some(qr) = &mut self.passport_qr {
                    qr.animation.restart();
                    qr.refresh();
                }
            }
            Message::LessDensePassportQr => {
                if let Some(qr) = &mut self.passport_qr {
                    if let Some(density) = qr.density.less_dense() {
                        if let Err(error) = qr.set_density(density) {
                            self.error = Some(Error::Unexpected(error));
                        }
                    }
                }
            }
            Message::MoreDensePassportQr => {
                if let Some(qr) = &mut self.passport_qr {
                    if let Some(density) = qr.density.more_dense() {
                        if let Err(error) = qr.set_density(density) {
                            self.error = Some(Error::Unexpected(error));
                        }
                    }
                }
            }
            Message::ExportPassportRegistration => {
                let Some(descriptor) = self.descriptor.as_ref() else {
                    return Task::none();
                };
                let bytes = match PolicyRegistration::from_descriptor(
                    wallet_name(descriptor),
                    self.network,
                    descriptor,
                )
                .and_then(|registration| {
                    AirgappedRequest::RegisterPolicy(registration)
                        .encode()
                        .map(|payload| payload.data)
                }) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.error = Some(Error::Unexpected(error.to_string()));
                        return Task::none();
                    }
                };
                return Task::perform(
                    async move {
                        let Some(path) = get_path("liana-policy.json".to_owned(), true).await
                        else {
                            return Ok(None);
                        };
                        fs::write(&path, bytes)
                            .map(|_| Some(path))
                            .map_err(|error| error.to_string())
                    },
                    Message::PassportRegistrationFileExported,
                );
            }
            Message::PassportRegistrationFileExported(result) => match result {
                Ok(Some(_)) => {
                    self.error = None;
                }
                Ok(None) => {}
                Err(error) => self.error = Some(Error::Unexpected(error)),
            },
            Message::PassportRegistrationExported(fingerprint) => {
                if self.passport_qr.as_ref().map(|qr| qr.fingerprint) == Some(fingerprint) {
                    self.registered.insert(fingerprint);
                    self.passport_qr = None;
                }
            }
            Message::CancelPassportRegistration => self.passport_qr = None,
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
        if let Some(descriptor) = self.descriptor.as_ref() {
            let checksum = descriptor
                .to_string()
                .rsplit_once('#')
                .map(|(_, checksum)| checksum.to_owned())
                .unwrap_or_default();
            for signer in ctx.airgapped_signers.values_mut() {
                if self.registered.contains(&signer.fingerprint) {
                    signer.registration = crate::airgap::RegistrationState::Exported {
                        descriptor_checksum: checksum.clone(),
                    };
                }
            }
        }
        true
    }
    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        let hws = hws.refresh().map(Message::HardwareWallets);
        let qr = if self.passport_qr.is_some() {
            iced::time::every(std::time::Duration::from_millis(50)).map(|_| Message::PassportQrTick)
        } else {
            Subscription::none()
        };
        Subscription::batch(vec![hws, qr])
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

        view::register_descriptor(
            progress,
            network,
            email,
            desc,
            &hws.list,
            &self.airgapped_signers,
            self.passport_qr.as_ref().map(|qr| {
                let state = qr.animation.state();
                (
                    qr.fingerprint,
                    &qr.qr_data,
                    state.frame,
                    state.total_frames,
                    state.paused,
                    qr.density,
                )
            }),
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
    help_open: bool,
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
                            let descriptor = descriptor.ok_or(encrypted_backup::Error::String(
                                Box::new("Descriptor missing".to_string()),
                            ))?;
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
                    let modal =
                        ExportModal::new(None, ImportExportType::ExportEncryptedDescriptor(bytes));
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
