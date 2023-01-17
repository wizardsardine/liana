use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;

use iced::{Command, Element};
use liana::{
    descriptors::{LianaDescKeys, MultipathDescriptor},
    miniscript::{
        bitcoin::{
            util::bip32::{DerivationPath, ExtendedPubKey, Fingerprint},
            Network,
        },
        descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
    },
};

use crate::{
    hw::{list_hardware_wallets, HardwareWallet},
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view, Error,
    },
    ui::component::{form, modal::Modal},
};

const LIANA_STANDARD_PATH: &str = "m/48'/0'/0'/2'";
const LIANA_TESTNET_STANDARD_PATH: &str = "m/48'/1'/0'/2'";

pub trait DescriptorKeyModal {
    fn processing(&self) -> bool {
        false
    }
    fn update(&mut self, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<Message>;
}

pub struct DefineDescriptor {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,
    spending_keys: Vec<DescriptorKey>,
    spending_threshold: usize,
    recovery_keys: Vec<DescriptorKey>,
    recovery_threshold: usize,
    sequence: form::Value<String>,
    modal: Option<Box<dyn DescriptorKeyModal>>,

    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            data_dir: None,
            network_valid: true,
            spending_keys: vec![DescriptorKey::new("Key 1".to_string())],
            spending_threshold: 1,
            recovery_keys: vec![DescriptorKey::new("Recovery key 1".to_string())],
            recovery_threshold: 1,
            sequence: form::Value::default(),
            modal: None,
            error: None,
        }
    }

    fn valid(&self) -> bool {
        !self.spending_keys.is_empty()
            && !self.recovery_keys.is_empty()
            && !self.sequence.value.is_empty()
            && !self.spending_keys.iter().any(|k| k.key.is_none())
            && !self.spending_keys.iter().any(|k| k.key.is_none())
    }

    // TODO: Improve algo
    fn check_for_duplicate(&mut self) {
        let mut all_keys = HashSet::new();
        let mut duplicate_keys = HashSet::new();
        let mut all_names = HashSet::new();
        let mut duplicate_names = HashSet::new();
        for spending_key in &self.spending_keys {
            if all_names.contains(&spending_key.name) {
                duplicate_names.insert(spending_key.name.clone());
            } else {
                all_names.insert(spending_key.name.clone());
            }
            if let Some(key) = &spending_key.key {
                if all_keys.contains(key) {
                    duplicate_keys.insert(key.clone());
                } else {
                    all_keys.insert(key.clone());
                }
            }
        }
        for recovery_key in &self.recovery_keys {
            if all_names.contains(&recovery_key.name) {
                duplicate_names.insert(recovery_key.name.clone());
            } else {
                all_names.insert(recovery_key.name.clone());
            }
            if let Some(key) = &recovery_key.key {
                if all_keys.contains(key) {
                    duplicate_keys.insert(key.clone());
                } else {
                    all_keys.insert(key.clone());
                }
            }
        }
        for spending_key in self.spending_keys.iter_mut() {
            spending_key.duplicate_name = duplicate_names.contains(&spending_key.name);
            if let Some(key) = &spending_key.key {
                spending_key.duplicate_key = duplicate_keys.contains(&key);
            }
        }
        for recovery_key in self.recovery_keys.iter_mut() {
            if let Some(key) = &recovery_key.key {
                recovery_key.duplicate_key = duplicate_keys.contains(&key);
            }
        }
    }
}

impl Step for DefineDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) -> Command<Message> {
        self.error = None;
        match message {
            Message::Close => {
                self.modal = None;
            }
            Message::Network(network) => {
                self.network = network;
                let mut network_datadir = self.data_dir.clone().unwrap();
                network_datadir.push(self.network.to_string());
                self.network_valid = !network_datadir.exists();
                for key in self.spending_keys.iter_mut() {
                    key.check_network(self.network);
                }
                for key in self.recovery_keys.iter_mut() {
                    key.check_network(self.network);
                }
            }
            Message::DefineDescriptor(msg) => {
                match msg {
                    message::DefineDescriptor::ThresholdEdited(is_recovery, value) => {
                        if is_recovery {
                            self.recovery_threshold = value;
                        } else {
                            self.spending_threshold = value;
                        }
                    }
                    message::DefineDescriptor::SequenceEdited(seq) => {
                        self.sequence.valid = true;
                        if seq.is_empty() || seq.parse::<u16>().is_ok() {
                            self.sequence.value = seq;
                        }
                    }
                    message::DefineDescriptor::AddKey(is_recovery) => {
                        if is_recovery {
                            self.recovery_keys.push(DescriptorKey::new(format!(
                                "Recovery key {}",
                                self.recovery_keys.len() + 1
                            )));
                            self.recovery_threshold += 1;
                        } else {
                            self.spending_keys.push(DescriptorKey::new(format!(
                                "Key {}",
                                self.spending_keys.len() + 1
                            )));
                            self.spending_threshold += 1;
                        }
                    }
                    message::DefineDescriptor::Key(is_recovery, i, msg) => match msg {
                        message::DefineKey::Clipboard(key) => {
                            return Command::perform(async move { key }, Message::Clibpboard);
                        }
                        message::DefineKey::Imported(imported_key) => {
                            if is_recovery {
                                if let Some(recovery_key) = self.recovery_keys.get_mut(i) {
                                    recovery_key.key = Some(imported_key);
                                    recovery_key.check_network(self.network);
                                }
                            } else if let Some(spending_key) = self.spending_keys.get_mut(i) {
                                spending_key.key = Some(imported_key);
                                spending_key.check_network(self.network);
                            }
                            self.modal = None;
                            self.check_for_duplicate();
                        }
                        message::DefineKey::ImportFromClipboard => {
                            let modal = ImportXpubModal::new(i, is_recovery, self.network);
                            self.modal = Some(Box::new(modal));
                        }
                        message::DefineKey::ImportFromHardware => {
                            let modal = HardwareXpubModal::new(i, is_recovery, self.network);
                            let cmd = modal.load();
                            self.modal = Some(Box::new(modal));
                            return cmd;
                        }
                        message::DefineKey::Delete => {
                            if is_recovery {
                                self.recovery_keys.remove(i);
                                if self.recovery_threshold > self.recovery_keys.len() {
                                    self.recovery_threshold -= 1;
                                }
                            } else {
                                self.spending_keys.remove(i);
                                if self.spending_threshold > self.spending_keys.len() {
                                    self.spending_threshold -= 1;
                                }
                            }
                            self.check_for_duplicate();
                        }
                    },
                    _ => {
                        if let Some(modal) = &mut self.modal {
                            return modal.update(Message::DefineDescriptor(msg));
                        }
                    }
                };
            }
            _ => {
                if let Some(modal) = &mut self.modal {
                    return modal.update(message);
                }
            }
        };
        Command::none()
    }

    fn load_context(&mut self, ctx: &Context) {
        self.network = ctx.bitcoin_config.network;
        self.data_dir = Some(ctx.data_dir.clone());
        let mut network_datadir = ctx.data_dir.clone();
        network_datadir.push(self.network.to_string());
        self.network_valid = !network_datadir.exists();
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        let spending_keys: Vec<DescriptorPublicKey> = self
            .spending_keys
            .iter()
            .filter_map(|k| k.key.clone())
            .collect();

        let recovery_keys: Vec<DescriptorPublicKey> = self
            .recovery_keys
            .iter()
            .filter_map(|k| k.key.clone())
            .collect();

        let sequence = self.sequence.value.parse::<u16>();
        self.sequence.valid = sequence.is_ok();

        if !self.network_valid
            || !self.sequence.valid
            || recovery_keys.is_empty()
            || spending_keys.is_empty()
        {
            return false;
        }

        let spending_keys = if spending_keys.len() == 1 {
            LianaDescKeys::from_single(spending_keys[0].clone())
        } else {
            match LianaDescKeys::from_multi(self.spending_threshold, spending_keys) {
                Ok(keys) => keys,
                Err(e) => {
                    self.error = Some(e.to_string());
                    return false;
                }
            }
        };

        let recovery_keys = if recovery_keys.len() == 1 {
            LianaDescKeys::from_single(recovery_keys[0].clone())
        } else {
            match LianaDescKeys::from_multi(self.recovery_threshold, recovery_keys) {
                Ok(keys) => keys,
                Err(e) => {
                    self.error = Some(e.to_string());
                    return false;
                }
            }
        };

        let desc = match MultipathDescriptor::new(spending_keys, recovery_keys, sequence.unwrap()) {
            Ok(desc) => desc,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };

        ctx.descriptor = Some(desc);
        true
    }

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        let content = view::define_descriptor(
            progress,
            self.network,
            self.network_valid,
            self.spending_keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    key.view().map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::Key(false, i, msg))
                    })
                })
                .collect(),
            self.recovery_keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    key.view().map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::Key(true, i, msg))
                    })
                })
                .collect(),
            &self.sequence,
            self.spending_threshold,
            self.recovery_threshold,
            self.valid(),
            self.error.as_ref(),
        );
        if let Some(modal) = &self.modal {
            Modal::new(content, modal.view())
                .on_blur(if modal.processing() {
                    None
                } else {
                    Some(Message::Close)
                })
                .into()
        } else {
            content
        }
    }
}

pub struct DescriptorKey {
    pub name: String,
    pub valid: bool,
    pub key: Option<DescriptorPublicKey>,
    pub duplicate_key: bool,
    pub duplicate_name: bool,
}

impl DescriptorKey {
    pub fn new(name: String) -> Self {
        Self {
            name,
            valid: true,
            key: None,
            duplicate_key: false,
            duplicate_name: false,
        }
    }

    pub fn check_network(&mut self, network: Network) {
        if let Some(key) = &self.key {
            self.valid = check_key_network(key, network);
        }
    }

    pub fn view(&self) -> Element<message::DefineKey> {
        match &self.key {
            None => view::undefined_descriptor_key(),
            Some(key) => {
                view::defined_descriptor_key(key.to_string(), self.valid, self.duplicate_key)
            }
        }
    }
}

fn check_key_network(key: &DescriptorPublicKey, network: Network) -> bool {
    match key {
        DescriptorPublicKey::XPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        DescriptorPublicKey::MultiXPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        _ => true,
    }
}

impl Default for DefineDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl From<DefineDescriptor> for Box<dyn Step> {
    fn from(s: DefineDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct HardwareXpubModal {
    is_recovery: bool,
    key_index: usize,
    network: Network,
    error: Option<Error>,
    processing: bool,

    chosen_hw: Option<usize>,
    hws: Vec<HardwareWallet>,
}

impl HardwareXpubModal {
    fn new(key_index: usize, is_recovery: bool, network: Network) -> Self {
        Self {
            is_recovery,
            key_index,
            chosen_hw: None,
            processing: false,
            hws: Vec::new(),
            error: None,
            network,
        }
    }
    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
            Message::ConnectedHardwareWallets,
        )
    }
}

impl DescriptorKeyModal for HardwareXpubModal {
    fn processing(&self) -> bool {
        self.processing
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some(hw) = self.hws.get(i) {
                    let device = hw.device.clone();
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    return Command::perform(
                        get_extended_pubkey(device, hw.fingerprint, self.network),
                        |res| {
                            Message::DefineDescriptor(message::DefineDescriptor::HWXpubImported(
                                res,
                            ))
                        },
                    );
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                self.hws = hws;
            }
            Message::Reload => {
                return self.load();
            }
            Message::DefineDescriptor(message::DefineDescriptor::HWXpubImported(res)) => {
                self.processing = false;
                match res {
                    Ok(key) => {
                        let key_index = self.key_index;
                        let is_recovery = self.is_recovery;
                        return Command::perform(
                            async move { (is_recovery, key_index, key) },
                            |(is_recovery, key_index, key)| {
                                message::DefineDescriptor::Key(
                                    is_recovery,
                                    key_index,
                                    message::DefineKey::Imported(key),
                                )
                            },
                        )
                        .map(Message::DefineDescriptor);
                    }
                    Err(e) => {
                        self.error = Some(e);
                    }
                }
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<Message> {
        view::hardware_wallet_xpubs_modal(
            self.is_recovery,
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
        )
    }
}

pub struct ImportXpubModal {
    is_recovery: bool,
    key_index: usize,
    form_xpub: form::Value<String>,
    network: Network,
}

impl ImportXpubModal {
    fn new(key_index: usize, is_recovery: bool, network: Network) -> Self {
        Self {
            form_xpub: form::Value::default(),
            is_recovery,
            key_index,
            network,
        }
    }
}

impl DescriptorKeyModal for ImportXpubModal {
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::DefineDescriptor(message::DefineDescriptor::XPubEdited(s)) => {
                self.form_xpub.valid =
                    DescriptorPublicKey::from_str(&format!("{}/<0;1>/*", s)).is_ok();
                self.form_xpub.value = s;
            }
            Message::DefineDescriptor(message::DefineDescriptor::ConfirmXpub) => {
                if let Ok(key) =
                    DescriptorPublicKey::from_str(&format!("{}/<0;1>/*", self.form_xpub.value))
                {
                    let key_index = self.key_index;
                    let is_recovery = self.is_recovery;
                    return Command::perform(
                        async move { (is_recovery, key_index, key) },
                        |(is_recovery, key_index, key)| {
                            message::DefineDescriptor::Key(
                                is_recovery,
                                key_index,
                                message::DefineKey::Imported(key),
                            )
                        },
                    )
                    .map(Message::DefineDescriptor);
                }
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<Message> {
        view::clipboard_xpub_modal(&self.form_xpub, self.network)
    }
}

pub struct XKey {
    origin: Option<(Fingerprint, DerivationPath)>,
    key: ExtendedPubKey,
}

impl std::fmt::Display for XKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some((ref master_id, ref master_deriv)) = self.origin {
            std::fmt::Formatter::write_str(f, "[")?;
            for byte in master_id.into_bytes().iter() {
                write!(f, "{:02x}", byte)?;
            }
            for child in master_deriv {
                write!(f, "/{}", child)?;
            }
            std::fmt::Formatter::write_str(f, "]")?;
        }
        self.key.fmt(f)?;
        Ok(())
    }
}

async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    network: Network,
) -> Result<DescriptorPublicKey, Error> {
    let derivation_path = DerivationPath::from_str(if network == Network::Bitcoin {
        LIANA_STANDARD_PATH
    } else {
        LIANA_TESTNET_STANDARD_PATH
    })
    .unwrap();
    let xkey = hw
        .get_extended_pubkey(&derivation_path, false)
        .await
        .map_err(Error::from)?;
    Ok(DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
        origin: Some((fingerprint, derivation_path)),
        derivation_paths: DerivPaths::new(vec![
            DerivationPath::from_str("m/0").unwrap(),
            DerivationPath::from_str("m/1").unwrap(),
        ])
        .unwrap(),
        wildcard: Wildcard::Unhardened,
        xkey,
    }))
}

pub struct ImportDescriptor {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,
    imported_descriptor: form::Value<String>,
    error: Option<String>,
}

impl ImportDescriptor {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            network_valid: true,
            data_dir: None,
            imported_descriptor: form::Value::default(),
            error: None,
        }
    }
}

impl Step for ImportDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Network(network) => {
                self.network = network;
                let mut network_datadir = self.data_dir.clone().unwrap();
                network_datadir.push(self.network.to_string());
                self.network_valid = !network_datadir.exists();
            }
            Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(desc)) => {
                self.imported_descriptor.value = desc;
                self.imported_descriptor.valid = true;
            }
            _ => {}
        };
        Command::none()
    }

    fn load_context(&mut self, ctx: &Context) {
        self.network = ctx.bitcoin_config.network;
        self.data_dir = Some(ctx.data_dir.clone());
        let mut network_datadir = ctx.data_dir.clone();
        network_datadir.push(self.network.to_string());
        self.network_valid = !network_datadir.exists();
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // descriptor forms for import or creation cannot be both empty or filled.
        if !self.imported_descriptor.value.is_empty() {
            if let Ok(desc) = MultipathDescriptor::from_str(&self.imported_descriptor.value) {
                self.imported_descriptor.valid = true;
                ctx.descriptor = Some(desc);
                true
            } else {
                self.imported_descriptor.valid = false;
                false
            }
        } else {
            false
        }
    }

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        view::import_descriptor(
            progress,
            self.network,
            self.network_valid,
            &self.imported_descriptor,
            self.error.as_ref(),
        )
    }
}

impl Default for ImportDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl From<ImportDescriptor> for Box<dyn Step> {
    fn from(s: ImportDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct RegisterDescriptor {
    descriptor: Option<MultipathDescriptor>,
    processing: bool,
    chosen_hw: Option<usize>,
    hws: Vec<(HardwareWallet, Option<[u8; 32]>, bool)>,
    error: Option<Error>,
}

impl Step for RegisterDescriptor {
    fn load_context(&mut self, ctx: &Context) {
        self.descriptor = ctx.descriptor.clone();
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some((hw, hmac, _)) = self.hws.get(i) {
                    if hmac.is_none() {
                        let device = hw.device.clone();
                        let descriptor = self.descriptor.as_ref().unwrap().to_string();
                        self.chosen_hw = Some(i);
                        self.processing = true;
                        self.error = None;
                        return Command::perform(
                            register_wallet(device, hw.fingerprint, descriptor),
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
                        if let Some(hw_h) = self
                            .hws
                            .iter_mut()
                            .find(|hw_h| hw_h.0.fingerprint == fingerprint)
                        {
                            hw_h.1 = hmac;
                            hw_h.2 = true;
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                for hw in hws {
                    if !self
                        .hws
                        .iter()
                        .any(|(h, _, _)| h.fingerprint == hw.fingerprint)
                    {
                        self.hws.push((hw, None, false));
                    }
                }
            }
            Message::Reload => {
                return self.load();
            }
            _ => {}
        };
        Command::none()
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        for (hw, token, registered) in &self.hws {
            if *registered {
                ctx.hws.push((hw.kind, hw.fingerprint, *token));
            }
        }
        true
    }
    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
            Message::ConnectedHardwareWallets,
        )
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        let desc = self.descriptor.as_ref().unwrap();
        view::register_descriptor(
            progress,
            desc.to_string(),
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
        )
    }
}

async fn register_wallet(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    descriptor: String,
) -> Result<(Fingerprint, Option<[u8; 32]>), Error> {
    let hmac = hw
        .register_wallet("Liana", &descriptor)
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
    descriptor: Option<MultipathDescriptor>,
}

impl Step for BackupDescriptor {
    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::BackupDone(done) = message {
            self.done = done;
        }
        Command::none()
    }
    fn load_context(&mut self, ctx: &Context) {
        self.descriptor = ctx.descriptor.clone();
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        let desc = self.descriptor.as_ref().unwrap();
        view::backup_descriptor(progress, desc.to_string(), self.done)
    }
}

impl From<BackupDescriptor> for Box<dyn Step> {
    fn from(s: BackupDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}
