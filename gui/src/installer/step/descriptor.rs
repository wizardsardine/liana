use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use iced::Command;
use liana::{
    descriptors::{LianaDescriptor, LianaPolicy, PathInfo},
    miniscript::{
        bitcoin::{
            bip32::{ChildNumber, DerivationPath, Fingerprint},
            Network,
        },
        descriptor::{
            DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, DescriptorXKey, Wildcard,
        },
    },
};

use liana_ui::{
    component::{form, modal::Modal},
    widget::Element,
};

use async_hwi::DeviceKind;

use crate::{
    app::settings::KeySetting,
    hw::{list_unregistered_hardware_wallets, HardwareWallet},
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view, Error,
    },
    signer::Signer,
};

pub trait DescriptorEditModal {
    fn processing(&self) -> bool {
        false
    }
    fn update(&mut self, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<Message>;
}

pub struct RecoveryPath {
    keys: Vec<DescriptorKey>,
    threshold: usize,
    sequence: u16,
    duplicate_sequence: bool,
}

impl RecoveryPath {
    pub fn new() -> Self {
        Self {
            keys: vec![DescriptorKey::default()],
            threshold: 1,
            sequence: u16::MAX,
            duplicate_sequence: false,
        }
    }

    fn valid(&self) -> bool {
        !self.keys.is_empty()
            && !self.keys.iter().any(|k| k.key.is_none())
            && !self.duplicate_sequence
    }

    fn check_network(&mut self, network: Network) {
        for key in self.keys.iter_mut() {
            key.check_network(network);
        }
    }

    fn view(&self) -> Element<message::DefinePath> {
        view::recovery_path_view(
            self.sequence,
            self.duplicate_sequence,
            self.threshold,
            self.keys
                .iter()
                .enumerate()
                .map(|(i, key)| key.view().map(move |msg| message::DefinePath::Key(i, msg)))
                .collect(),
        )
    }
}

pub struct DefineDescriptor {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,
    spending_keys: Vec<DescriptorKey>,
    spending_threshold: usize,
    recovery_paths: Vec<RecoveryPath>,

    modal: Option<Box<dyn DescriptorEditModal>>,
    signer: Arc<Mutex<Signer>>,

    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new(signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            network: Network::Bitcoin,
            data_dir: None,
            network_valid: true,
            spending_keys: vec![DescriptorKey::default()],
            spending_threshold: 1,
            recovery_paths: vec![RecoveryPath::new()],
            modal: None,
            signer,
            error: None,
        }
    }

    fn valid(&self) -> bool {
        !self.spending_keys.is_empty()
            && !self.spending_keys.iter().any(|k| k.key.is_none())
            && !self.recovery_paths.iter().any(|path| !path.valid())
    }

    fn set_network(&mut self, network: Network) {
        self.network = network;
        self.signer.lock().unwrap().set_network(network);
        if let Some(mut network_datadir) = self.data_dir.clone() {
            network_datadir.push(self.network.to_string());
            self.network_valid = !network_datadir.exists();
        }
        for key in self.spending_keys.iter_mut() {
            key.check_network(self.network);
        }
        for path in self.recovery_paths.iter_mut() {
            path.check_network(self.network);
        }
    }

    // TODO: Improve algo
    // Mark as duplicate every defined key that have the same name but not the same fingerprint.
    // And every undefined_key that have a same name than an other key.
    fn check_for_duplicate(&mut self) {
        let mut all_keys = HashSet::new();
        let mut duplicate_keys = HashSet::new();
        let mut all_names: HashMap<String, Fingerprint> = HashMap::new();
        let mut duplicate_names = HashSet::new();
        let mut all_sequence = HashSet::new();
        let mut duplicate_sequence = HashSet::new();
        for spending_key in &self.spending_keys {
            if let Some(key) = &spending_key.key {
                if let Some(fg) = all_names.get(&spending_key.name) {
                    if fg != &key.master_fingerprint() {
                        duplicate_names.insert(spending_key.name.clone());
                    }
                } else {
                    all_names.insert(spending_key.name.clone(), key.master_fingerprint());
                }
                if all_keys.contains(key) {
                    duplicate_keys.insert(key.clone());
                } else {
                    all_keys.insert(key.clone());
                }
            }
        }
        for path in &mut self.recovery_paths {
            if all_sequence.contains(&path.sequence) {
                duplicate_sequence.insert(path.sequence);
            } else {
                all_sequence.insert(path.sequence);
            }
            for recovery_key in &path.keys {
                if let Some(key) = &recovery_key.key {
                    if let Some(fg) = all_names.get(&recovery_key.name) {
                        if fg != &key.master_fingerprint() {
                            duplicate_names.insert(recovery_key.name.clone());
                        }
                    } else {
                        all_names.insert(recovery_key.name.clone(), key.master_fingerprint());
                    }
                    if all_keys.contains(key) {
                        duplicate_keys.insert(key.clone());
                    } else {
                        all_keys.insert(key.clone());
                    }
                }
            }
        }
        for spending_key in self.spending_keys.iter_mut() {
            spending_key.duplicate_name = duplicate_names.contains(&spending_key.name);
            if let Some(key) = &spending_key.key {
                spending_key.duplicate_key = duplicate_keys.contains(key);
            }
        }

        for path in &mut self.recovery_paths {
            path.duplicate_sequence = duplicate_sequence.contains(&path.sequence);
            for recovery_key in path.keys.iter_mut() {
                recovery_key.duplicate_name = duplicate_names.contains(&recovery_key.name);
                if let Some(key) = &recovery_key.key {
                    recovery_key.duplicate_key = duplicate_keys.contains(key);
                }
            }
        }
    }

    fn edit_alias_for_key_with_same_fingerprint(&mut self, name: String, fingerprint: Fingerprint) {
        for spending_key in &mut self.spending_keys {
            if spending_key.key.as_ref().map(|k| k.master_fingerprint()) == Some(fingerprint) {
                spending_key.name = name.clone();
            }
        }
        for path in &mut self.recovery_paths {
            for recovery_key in &mut path.keys {
                if recovery_key.key.as_ref().map(|k| k.master_fingerprint()) == Some(fingerprint) {
                    recovery_key.name = name.clone();
                }
            }
        }
    }

    /// Returns the maximum account index per key fingerprint
    fn fingerprint_account_index_mappping(&self) -> HashMap<Fingerprint, ChildNumber> {
        let mut mapping = HashMap::new();
        let update_mapping =
            |keys: &[DescriptorKey], mapping: &mut HashMap<Fingerprint, ChildNumber>| {
                for key in keys {
                    if let Some(DescriptorPublicKey::XPub(key)) = key.key.as_ref() {
                        if let Some((fingerprint, derivation_path)) = key.origin.as_ref() {
                            let index = if derivation_path.len() >= 4 {
                                if derivation_path[0].to_string() == "48'" {
                                    Some(derivation_path[2])
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(index) = index {
                                if let Some(previous_index) = mapping.get(fingerprint) {
                                    if index > *previous_index {
                                        mapping.insert(*fingerprint, index);
                                    }
                                } else {
                                    mapping.insert(*fingerprint, index);
                                }
                            }
                        }
                    }
                }
            };
        update_mapping(&self.spending_keys, &mut mapping);

        for path in &self.recovery_paths {
            update_mapping(&path.keys, &mut mapping);
        }
        mapping
    }

    fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for spending_key in &self.spending_keys {
            if let Some(key) = spending_key.key.as_ref() {
                map.insert(key.master_fingerprint(), spending_key.name.clone());
            }
        }
        for path in &self.recovery_paths {
            for recovery_key in &path.keys {
                if let Some(key) = recovery_key.key.as_ref() {
                    map.insert(key.master_fingerprint(), recovery_key.name.clone());
                }
            }
        }
        map
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
            Message::Network(network) => self.set_network(network),
            Message::DefineDescriptor(message::DefineDescriptor::AddRecoveryPath) => {
                self.recovery_paths.push(RecoveryPath::new());
            }
            Message::DefineDescriptor(message::DefineDescriptor::PrimaryPath(msg)) => match msg {
                message::DefinePath::ThresholdEdited(value) => {
                    self.spending_threshold = value;
                }
                message::DefinePath::AddKey => {
                    self.spending_keys.push(DescriptorKey::default());
                    self.spending_threshold += 1;
                }
                message::DefinePath::Key(i, msg) => match msg {
                    message::DefineKey::Clipboard(key) => {
                        return Command::perform(async move { key }, Message::Clibpboard);
                    }
                    message::DefineKey::Edited(name, imported_key, kind) => {
                        self.edit_alias_for_key_with_same_fingerprint(
                            name.clone(),
                            imported_key.master_fingerprint(),
                        );

                        if let Some(spending_key) = self.spending_keys.get_mut(i) {
                            spending_key.name = name;
                            spending_key.key = Some(imported_key);
                            spending_key.device_kind = kind;
                            spending_key.check_network(self.network);
                        }
                        self.modal = None;
                        self.check_for_duplicate();
                    }
                    message::DefineKey::Edit => {
                        if let Some(spending_key) = self.spending_keys.get(i) {
                            let modal = EditXpubModal::new(
                                spending_key.name.clone(),
                                spending_key.key.as_ref(),
                                None,
                                i,
                                self.network,
                                self.fingerprint_account_index_mappping(),
                                self.keys_aliases(),
                                self.signer.clone(),
                            );
                            let cmd = modal.load();
                            self.modal = Some(Box::new(modal));
                            return cmd;
                        }
                    }
                    message::DefineKey::Delete => {
                        self.spending_keys.remove(i);
                        if self.spending_threshold > self.spending_keys.len() {
                            self.spending_threshold -= 1;
                        }
                        self.check_for_duplicate();
                    }
                },
                _ => {}
            },
            Message::DefineDescriptor(message::DefineDescriptor::RecoveryPath(i, msg)) => match msg
            {
                message::DefinePath::ThresholdEdited(value) => {
                    if let Some(path) = self.recovery_paths.get_mut(i) {
                        path.threshold = value;
                    }
                }
                message::DefinePath::SequenceEdited(seq) => {
                    self.modal = None;
                    if let Some(path) = self.recovery_paths.get_mut(i) {
                        path.sequence = seq;
                    }
                    self.check_for_duplicate();
                }
                message::DefinePath::EditSequence => {
                    if let Some(path) = self.recovery_paths.get(i) {
                        self.modal = Some(Box::new(EditSequenceModal::new(i, path.sequence)));
                    }
                }
                message::DefinePath::AddKey => {
                    if let Some(path) = self.recovery_paths.get_mut(i) {
                        path.keys.push(DescriptorKey::default());
                        path.threshold += 1;
                    }
                }
                message::DefinePath::Key(j, msg) => match msg {
                    message::DefineKey::Clipboard(key) => {
                        return Command::perform(async move { key }, Message::Clibpboard);
                    }
                    message::DefineKey::Edited(name, imported_key, kind) => {
                        self.edit_alias_for_key_with_same_fingerprint(
                            name.clone(),
                            imported_key.master_fingerprint(),
                        );

                        if let Some(key) = self
                            .recovery_paths
                            .get_mut(i)
                            .and_then(|path| path.keys.get_mut(j))
                        {
                            key.name = name;
                            key.key = Some(imported_key);
                            key.device_kind = kind;
                            key.check_network(self.network);
                        }
                        self.modal = None;
                        self.check_for_duplicate();
                    }
                    message::DefineKey::Edit => {
                        if let Some(key) =
                            self.recovery_paths.get(i).and_then(|path| path.keys.get(j))
                        {
                            let modal = EditXpubModal::new(
                                key.name.clone(),
                                key.key.as_ref(),
                                Some(i),
                                j,
                                self.network,
                                self.fingerprint_account_index_mappping(),
                                self.keys_aliases(),
                                self.signer.clone(),
                            );
                            let cmd = modal.load();
                            self.modal = Some(Box::new(modal));
                            return cmd;
                        }
                    }
                    message::DefineKey::Delete => {
                        if let Some(path) = self.recovery_paths.get_mut(i) {
                            path.keys.remove(j);
                            if path.threshold > path.keys.len() {
                                path.threshold -= 1;
                            }
                        }
                        if self
                            .recovery_paths
                            .get(i)
                            .map(|path| path.keys.is_empty())
                            .unwrap_or(false)
                        {
                            self.recovery_paths.remove(i);
                        }
                        self.check_for_duplicate();
                    }
                },
            },
            _ => {
                if let Some(modal) = &mut self.modal {
                    return modal.update(message);
                }
            }
        };
        Command::none()
    }

    fn load_context(&mut self, ctx: &Context) {
        self.data_dir = Some(ctx.data_dir.clone());
        self.set_network(ctx.bitcoin_config.network)
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        ctx.keys = Vec::new();
        let mut hw_is_used = false;
        let mut spending_keys: Vec<DescriptorPublicKey> = Vec::new();
        for spending_key in self.spending_keys.iter().clone() {
            if let Some(DescriptorPublicKey::XPub(xpub)) = spending_key.key.as_ref() {
                if let Some((master_fingerprint, _)) = xpub.origin {
                    ctx.keys.push(KeySetting {
                        master_fingerprint,
                        name: spending_key.name.clone(),
                    });
                    if spending_key.device_kind.is_some() {
                        hw_is_used = true;
                    }
                }
                let xpub = DescriptorMultiXKey {
                    origin: xpub.origin.clone(),
                    xkey: xpub.xkey,
                    derivation_paths: DerivPaths::new(vec![
                        DerivationPath::from_str("m/0").unwrap(),
                        DerivationPath::from_str("m/1").unwrap(),
                    ])
                    .unwrap(),
                    wildcard: Wildcard::Unhardened,
                };
                spending_keys.push(DescriptorPublicKey::MultiXPub(xpub));
            }
        }

        let mut recovery_paths = BTreeMap::new();

        for path in self.recovery_paths.iter_mut() {
            let mut recovery_keys: Vec<DescriptorPublicKey> = Vec::new();
            for recovery_key in path.keys.iter().clone() {
                if let Some(DescriptorPublicKey::XPub(xpub)) = recovery_key.key.as_ref() {
                    if let Some((master_fingerprint, _)) = xpub.origin {
                        ctx.keys.push(KeySetting {
                            master_fingerprint,
                            name: recovery_key.name.clone(),
                        });
                        if recovery_key.device_kind.is_some() {
                            hw_is_used = true;
                        }
                    }
                    let xpub = DescriptorMultiXKey {
                        origin: xpub.origin.clone(),
                        xkey: xpub.xkey,
                        derivation_paths: DerivPaths::new(vec![
                            DerivationPath::from_str("m/0").unwrap(),
                            DerivationPath::from_str("m/1").unwrap(),
                        ])
                        .unwrap(),
                        wildcard: Wildcard::Unhardened,
                    };
                    recovery_keys.push(DescriptorPublicKey::MultiXPub(xpub));
                }
            }

            let recovery_keys = if recovery_keys.len() == 1 {
                PathInfo::Single(recovery_keys[0].clone())
            } else {
                PathInfo::Multi(path.threshold, recovery_keys)
            };

            recovery_paths.insert(path.sequence, recovery_keys);
        }

        if !self.network_valid || spending_keys.is_empty() {
            return false;
        }

        let spending_keys = if spending_keys.len() == 1 {
            PathInfo::Single(spending_keys[0].clone())
        } else {
            PathInfo::Multi(self.spending_threshold, spending_keys)
        };

        let policy = match LianaPolicy::new(spending_keys, recovery_paths) {
            Ok(policy) => policy,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };

        ctx.descriptor = Some(LianaDescriptor::new(policy));
        ctx.hw_is_used = hw_is_used;
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
                        Message::DefineDescriptor(message::DefineDescriptor::PrimaryPath(
                            message::DefinePath::Key(i, msg),
                        ))
                    })
                })
                .collect(),
            self.spending_threshold,
            self.recovery_paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    path.view().map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::RecoveryPath(i, msg))
                    })
                })
                .collect(),
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
    pub device_kind: Option<DeviceKind>,
    pub valid: bool,
    pub key: Option<DescriptorPublicKey>,
    pub duplicate_key: bool,
    pub duplicate_name: bool,
}

impl Default for DescriptorKey {
    fn default() -> Self {
        Self {
            name: "".to_string(),
            device_kind: None,
            valid: true,
            key: None,
            duplicate_key: false,
            duplicate_name: false,
        }
    }
}

impl DescriptorKey {
    pub fn check_network(&mut self, network: Network) {
        if let Some(key) = &self.key {
            self.valid = check_key_network(key, network);
        }
    }

    pub fn view(&self) -> Element<message::DefineKey> {
        match &self.key {
            None => view::undefined_descriptor_key(),
            Some(_) => view::defined_descriptor_key(
                &self.name,
                self.valid,
                self.duplicate_key,
                self.duplicate_name,
            ),
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

impl From<DefineDescriptor> for Box<dyn Step> {
    fn from(s: DefineDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct EditSequenceModal {
    path_index: usize,
    sequence: form::Value<String>,
}

impl EditSequenceModal {
    pub fn new(path_index: usize, sequence: u16) -> Self {
        Self {
            path_index,
            sequence: form::Value {
                value: sequence.to_string(),
                valid: true,
            },
        }
    }
}

impl DescriptorEditModal for EditSequenceModal {
    fn processing(&self) -> bool {
        false
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::DefineDescriptor(message::DefineDescriptor::SequenceModal(msg)) = message {
            match msg {
                message::SequenceModal::SequenceEdited(seq) => {
                    if let Ok(s) = u16::from_str(&seq) {
                        self.sequence.valid = s != 0
                    } else {
                        self.sequence.valid = false;
                    }
                    self.sequence.value = seq;
                }
                message::SequenceModal::ConfirmSequence => {
                    if self.sequence.valid {
                        if let Ok(sequence) = u16::from_str(&self.sequence.value) {
                            let path_index = self.path_index;
                            return Command::perform(
                                async move { (path_index, sequence) },
                                |(path_index, sequence)| {
                                    message::DefineDescriptor::RecoveryPath(
                                        path_index,
                                        message::DefinePath::SequenceEdited(sequence),
                                    )
                                },
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                }
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<Message> {
        view::edit_sequence_modal(&self.sequence)
    }
}

pub struct EditXpubModal {
    /// None if path is primary path
    path_index: Option<usize>,
    key_index: usize,
    network: Network,
    error: Option<Error>,
    processing: bool,

    keys_aliases: HashMap<Fingerprint, String>,
    account_indexes: HashMap<Fingerprint, ChildNumber>,

    form_name: form::Value<String>,
    form_xpub: form::Value<String>,
    edit_name: bool,

    hws: Vec<HardwareWallet>,
    hot_signer: Arc<Mutex<Signer>>,
    hot_signer_fingerprint: Fingerprint,
    chosen_signer: Option<(Fingerprint, Option<DeviceKind>)>,
}

impl EditXpubModal {
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        key: Option<&DescriptorPublicKey>,
        path_index: Option<usize>,
        key_index: usize,
        network: Network,
        account_indexes: HashMap<Fingerprint, ChildNumber>,
        keys_aliases: HashMap<Fingerprint, String>,
        hot_signer: Arc<Mutex<Signer>>,
    ) -> Self {
        let hot_signer_fingerprint = hot_signer.lock().unwrap().fingerprint();
        Self {
            form_name: form::Value {
                valid: true,
                value: name,
            },
            form_xpub: form::Value {
                valid: true,
                value: key.map(|k| k.to_string()).unwrap_or_else(String::new),
            },
            keys_aliases,
            account_indexes,
            path_index,
            key_index,
            processing: false,
            hws: Vec::new(),
            error: None,
            network,
            edit_name: false,
            chosen_signer: key.map(|k| (k.master_fingerprint(), None)),
            hot_signer_fingerprint,
            hot_signer,
        }
    }
    fn load(&self) -> Command<Message> {
        let keys_aliases = self.keys_aliases.clone();
        Command::perform(
            async move { list_unregistered_hardware_wallets(Some(&keys_aliases)).await },
            Message::ConnectedHardwareWallets,
        )
    }
}

impl DescriptorEditModal for EditXpubModal {
    fn processing(&self) -> bool {
        self.processing
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    kind,
                    ..
                }) = self.hws.get(i)
                {
                    self.chosen_signer = Some((*fingerprint, Some(*kind)));
                    self.processing = true;
                    // If another account n exists, the key is retrieved for the account n+1
                    let account_index = self
                        .account_indexes
                        .get(fingerprint)
                        .map(|account_index| account_index.increment().unwrap())
                        .unwrap_or_else(|| ChildNumber::from_hardened_idx(0).unwrap());
                    return Command::perform(
                        get_extended_pubkey(
                            device.clone(),
                            *fingerprint,
                            generate_derivation_path(self.network, account_index),
                        ),
                        |res| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::HWXpubImported(res),
                            ))
                        },
                    );
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                if let Ok(key) = DescriptorPublicKey::from_str(&self.form_xpub.value) {
                    self.chosen_signer = Some((
                        key.master_fingerprint(),
                        hws.iter()
                            .find(|hw| hw.fingerprint() == Some(key.master_fingerprint()))
                            .map(|hw| *hw.kind()),
                    ));
                }
                self.hws = hws;
            }
            Message::Reload => {
                self.hws = Vec::new();
                return self.load();
            }
            Message::UseHotSigner => {
                let fingerprint = self.hot_signer.lock().unwrap().fingerprint();
                self.chosen_signer = Some((fingerprint, None));
                self.form_xpub.valid = true;
                if let Some(alias) = self.keys_aliases.get(&fingerprint) {
                    self.form_name.valid = true;
                    self.form_name.value = alias.clone();
                    self.edit_name = false;
                } else {
                    self.edit_name = true;
                    self.form_name.value = String::new();
                }
                let account_index = self
                    .account_indexes
                    .get(&fingerprint)
                    .map(|account_index| account_index.increment().unwrap())
                    .unwrap_or_else(|| ChildNumber::from_hardened_idx(0).unwrap());
                let derivation_path = generate_derivation_path(self.network, account_index);
                self.form_xpub.value = format!(
                    "[{}{}]{}",
                    fingerprint,
                    derivation_path.to_string().trim_start_matches('m'),
                    self.hot_signer
                        .lock()
                        .unwrap()
                        .get_extended_pubkey(&derivation_path)
                );
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(msg)) => match msg {
                message::ImportKeyModal::HWXpubImported(res) => {
                    self.processing = false;
                    match res {
                        Ok(key) => {
                            if let Some(alias) = self.keys_aliases.get(&key.master_fingerprint()) {
                                self.form_name.valid = true;
                                self.form_name.value = alias.clone();
                                self.edit_name = false;
                            } else {
                                self.edit_name = true;
                                self.form_name.value = String::new();
                            }
                            self.form_xpub.valid = true;
                            self.form_xpub.value = key.to_string();
                        }
                        Err(e) => {
                            self.chosen_signer = None;
                            self.error = Some(e);
                        }
                    }
                }
                message::ImportKeyModal::EditName => {
                    self.edit_name = true;
                }
                message::ImportKeyModal::NameEdited(name) => {
                    self.form_name.valid = true;
                    self.form_name.value = name;
                }
                message::ImportKeyModal::XPubEdited(s) => {
                    if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&s) {
                        self.chosen_signer = None;
                        if let Some((fingerprint, _)) = key.origin {
                            self.form_xpub.valid = true;
                            if let Some(alias) = self.keys_aliases.get(&fingerprint) {
                                self.form_name.valid = true;
                                self.form_name.value = alias.clone();
                                self.edit_name = false;
                            } else {
                                self.edit_name = true;
                            }
                        } else {
                            self.form_xpub.valid = false;
                        }
                    } else {
                        self.form_xpub.valid = false;
                    }
                    self.form_xpub.value = s;
                }
                message::ImportKeyModal::ConfirmXpub => {
                    if let Ok(key) = DescriptorPublicKey::from_str(&self.form_xpub.value) {
                        let key_index = self.key_index;
                        let name = self.form_name.value.clone();
                        let device_kind = self.chosen_signer.and_then(|(_, kind)| kind);
                        if let Some(path_index) = self.path_index {
                            return Command::perform(
                                async move { (path_index, key_index, key) },
                                move |(path_index, key_index, key)| {
                                    message::DefineDescriptor::RecoveryPath(
                                        path_index,
                                        message::DefinePath::Key(
                                            key_index,
                                            message::DefineKey::Edited(name, key, device_kind),
                                        ),
                                    )
                                },
                            )
                            .map(Message::DefineDescriptor);
                        } else {
                            return Command::perform(
                                async move { (key_index, key) },
                                move |(key_index, key)| {
                                    message::DefineDescriptor::PrimaryPath(
                                        message::DefinePath::Key(
                                            key_index,
                                            message::DefineKey::Edited(name, key, device_kind),
                                        ),
                                    )
                                },
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                }
            },
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<Message> {
        view::edit_key_modal(
            self.network,
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_signer.map(|s| s.0),
            &self.hot_signer_fingerprint,
            self.keys_aliases.get(&self.hot_signer_fingerprint),
            &self.form_xpub,
            &self.form_name,
            self.edit_name,
        )
    }
}

fn generate_derivation_path(network: Network, account_index: ChildNumber) -> DerivationPath {
    DerivationPath::from_str(&{
        if network == Network::Bitcoin {
            format!("m/48'/0'/{}/2'", account_index)
        } else {
            format!("m/48'/1'/{}/2'", account_index)
        }
    })
    .unwrap()
}

/// LIANA_STANDARD_PATH: m/48'/0'/0'/2';
/// LIANA_TESTNET_STANDARD_PATH: m/48'/1'/0'/2';
async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    derivation_path: DerivationPath,
) -> Result<DescriptorPublicKey, Error> {
    let xkey = hw
        .get_extended_pubkey(&derivation_path)
        .await
        .map_err(Error::from)?;
    Ok(DescriptorPublicKey::XPub(DescriptorXKey {
        origin: Some((fingerprint, derivation_path)),
        derivation_path: DerivationPath::master(),
        wildcard: Wildcard::None,
        xkey,
    }))
}

pub struct HardwareWalletXpubs {
    hw: HardwareWallet,
    xpubs: Vec<String>,
    processing: bool,
    error: Option<Error>,
    next_account: ChildNumber,
}

impl HardwareWalletXpubs {
    fn new(hw: HardwareWallet) -> Self {
        Self {
            hw,
            xpubs: Vec::new(),
            processing: false,
            error: None,
            next_account: ChildNumber::from_hardened_idx(0).unwrap(),
        }
    }

    fn update(&mut self, res: Result<DescriptorPublicKey, Error>) {
        self.processing = false;
        match res {
            Err(e) => {
                self.error = e.into();
            }
            Ok(xpub) => {
                self.error = None;
                self.next_account = self.next_account.increment().unwrap();
                self.xpubs.push(xpub.to_string());
            }
        }
    }

    fn reset(&mut self) {
        self.error = None;
        self.next_account = ChildNumber::from_hardened_idx(0).unwrap();
        self.xpubs = Vec::new();
    }

    fn select(&mut self, i: usize, network: Network) -> Command<Message> {
        if let HardwareWallet::Supported {
            device,
            fingerprint,
            ..
        } = &self.hw
        {
            let device = device.clone();
            let fingerprint = *fingerprint;
            self.processing = true;
            self.error = None;
            let derivation_path = generate_derivation_path(network, self.next_account);
            Command::perform(
                async move {
                    (
                        i,
                        get_extended_pubkey(device, fingerprint, derivation_path).await,
                    )
                },
                |(i, res)| Message::ImportXpub(i, res),
            )
        } else {
            Command::none()
        }
    }

    pub fn view(&self, i: usize) -> Element<Message> {
        view::hardware_wallet_xpubs(
            i,
            &self.xpubs,
            &self.hw,
            self.processing,
            self.error.as_ref(),
        )
    }
}

pub struct SignerXpubs {
    signer: Arc<Mutex<Signer>>,
    xpubs: Vec<String>,
    next_account: ChildNumber,
}

impl SignerXpubs {
    fn new(signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            signer,
            xpubs: Vec::new(),
            next_account: ChildNumber::from_hardened_idx(0).unwrap(),
        }
    }

    fn reset(&mut self) {
        self.xpubs = Vec::new();
        self.next_account = ChildNumber::from_hardened_idx(0).unwrap();
    }

    fn select(&mut self, network: Network) {
        let derivation_path = generate_derivation_path(network, self.next_account);
        self.next_account = self.next_account.increment().unwrap();
        let signer = self.signer.lock().unwrap();
        self.xpubs.push(format!(
            "[{}{}]{}",
            signer.fingerprint(),
            derivation_path.to_string().trim_start_matches('m'),
            signer.get_extended_pubkey(&derivation_path)
        ));
    }

    pub fn view(&self) -> Element<Message> {
        view::signer_xpubs(&self.xpubs)
    }
}

pub struct ParticipateXpub {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,

    shared: bool,

    xpubs_hw: Vec<HardwareWalletXpubs>,
    xpubs_signer: SignerXpubs,
}

impl ParticipateXpub {
    pub fn new(signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            network: Network::Bitcoin,
            network_valid: true,
            data_dir: None,
            xpubs_hw: Vec::new(),
            shared: false,
            xpubs_signer: SignerXpubs::new(signer),
        }
    }

    fn set_network(&mut self, network: Network) {
        if network != self.network {
            self.xpubs_hw.iter_mut().for_each(|hw| hw.reset());
            self.xpubs_signer.reset();
        }
        self.network = network;
        self.xpubs_signer
            .signer
            .lock()
            .unwrap()
            .set_network(network);
        if let Some(mut network_datadir) = self.data_dir.clone() {
            network_datadir.push(self.network.to_string());
            self.network_valid = !network_datadir.exists();
        }
    }
}

impl Step for ParticipateXpub {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Network(network) => {
                self.set_network(network);
            }
            Message::UserActionDone(shared) => self.shared = shared,
            Message::ImportXpub(i, res) => {
                if let Some(hw) = self.xpubs_hw.get_mut(i) {
                    hw.update(res);
                }
            }
            Message::UseHotSigner => {
                self.xpubs_signer.select(self.network);
            }
            Message::Select(i) => {
                if let Some(hw) = self.xpubs_hw.get_mut(i) {
                    return hw.select(i, self.network);
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                for hw in hws {
                    if let Some(xpub_hw) = self.xpubs_hw.iter_mut().find(|h| {
                        h.hw.kind() == hw.kind()
                            && (h.hw.fingerprint() == hw.fingerprint() || !h.hw.is_supported())
                    }) {
                        xpub_hw.hw = hw;
                    } else {
                        self.xpubs_hw.push(HardwareWalletXpubs::new(hw));
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

    fn load_context(&mut self, ctx: &Context) {
        self.data_dir = Some(ctx.data_dir.clone());
        self.set_network(ctx.bitcoin_config.network);
    }

    fn load(&self) -> Command<Message> {
        Command::perform(
            list_unregistered_hardware_wallets(None),
            Message::ConnectedHardwareWallets,
        )
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        // Drop connections to hardware wallets.
        self.xpubs_hw = Vec::new();
        true
    }

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        view::participate_xpub(
            progress,
            self.network,
            self.network_valid,
            self.xpubs_hw
                .iter()
                .enumerate()
                .map(|(i, hw)| hw.view(i))
                .collect(),
            self.xpubs_signer.view(),
            self.shared,
        )
    }
}

impl From<ParticipateXpub> for Box<dyn Step> {
    fn from(s: ParticipateXpub) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct ImportDescriptor {
    network: Network,
    network_valid: bool,
    change_network: bool,
    data_dir: Option<PathBuf>,
    imported_descriptor: form::Value<String>,
    error: Option<String>,
}

impl ImportDescriptor {
    pub fn new(change_network: bool) -> Self {
        Self {
            change_network,
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
        // Set to true in order to force the registration process to be shown to user.
        ctx.hw_is_used = true;
        // descriptor forms for import or creation cannot be both empty or filled.
        if !self.imported_descriptor.value.is_empty() {
            if let Ok(desc) = LianaDescriptor::from_str(&self.imported_descriptor.value) {
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
            self.change_network,
            self.network,
            self.network_valid,
            &self.imported_descriptor,
            self.error.as_ref(),
        )
    }
}

impl From<ImportDescriptor> for Box<dyn Step> {
    fn from(s: ImportDescriptor) -> Box<dyn Step> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct RegisterDescriptor {
    descriptor: Option<LianaDescriptor>,
    keys_aliases: HashMap<Fingerprint, String>,
    processing: bool,
    chosen_hw: Option<usize>,
    hws: Vec<HardwareWallet>,
    hmacs: Vec<(Fingerprint, DeviceKind, Option<[u8; 32]>)>,
    registered: HashSet<Fingerprint>,
    error: Option<Error>,
    done: bool,
}

impl Step for RegisterDescriptor {
    fn load_context(&mut self, ctx: &Context) {
        self.descriptor = ctx.descriptor.clone();
        let mut map = HashMap::new();
        for key in ctx.keys.iter().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
        self.keys_aliases = map;
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    ..
                }) = self.hws.get(i)
                {
                    if !self.registered.contains(fingerprint) {
                        let descriptor = self.descriptor.as_ref().unwrap().to_string();
                        self.chosen_hw = Some(i);
                        self.processing = true;
                        self.error = None;
                        return Command::perform(
                            register_wallet(device.clone(), *fingerprint, descriptor),
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
                            .iter()
                            .find(|hw_h| hw_h.fingerprint() == Some(fingerprint))
                        {
                            self.registered.insert(fingerprint);
                            self.hmacs.push((fingerprint, *hw_h.kind(), hmac));
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::ConnectedHardwareWallets(hws) => {
                self.hws = hws;
            }
            Message::Reload => {
                self.hws = Vec::new();
                return self.load();
            }
            Message::UserActionDone(done) => {
                self.done = done;
            }
            _ => {}
        };
        Command::none()
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
    fn load(&self) -> Command<Message> {
        let keys_aliases = self.keys_aliases.clone();
        Command::perform(
            async move { list_unregistered_hardware_wallets(Some(&keys_aliases)).await },
            Message::ConnectedHardwareWallets,
        )
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        let desc = self.descriptor.as_ref().unwrap();
        view::register_descriptor(
            progress,
            desc.to_string(),
            &self.hws,
            &self.registered,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
            self.done,
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
    descriptor: Option<LianaDescriptor>,
}

impl Step for BackupDescriptor {
    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::UserActionDone(done) = message {
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced_native::command::Action;
    use std::sync::{Arc, Mutex};

    pub struct Sandbox<S: Step> {
        step: Arc<Mutex<S>>,
    }

    impl<S: Step + 'static> Sandbox<S> {
        pub fn new(step: S) -> Self {
            Self {
                step: Arc::new(Mutex::new(step)),
            }
        }

        pub fn check<F: FnOnce(&mut S)>(&self, check: F) {
            let mut step = self.step.lock().unwrap();
            check(&mut step)
        }

        pub async fn update(&self, message: Message) {
            let cmd = self.step.lock().unwrap().update(message);
            for action in cmd.actions() {
                if let Action::Future(f) = action {
                    let msg = f.await;
                    let _cmd = self.step.lock().unwrap().update(msg);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_define_descriptor_use_hotkey() {
        let mut ctx = Context::new(Network::Signet, PathBuf::from_str("/").unwrap());
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(Arc::new(
            Mutex::new(Signer::generate(Network::Bitcoin).unwrap()),
        )));

        // Edit primary key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::PrimaryPath(message::DefinePath::Key(
                    0,
                    message::DefineKey::Edit,
                )),
            ))
            .await;
        sandbox.check(|step| assert!(step.modal.is_some()));
        sandbox.update(Message::UseHotSigner).await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::NameEdited(
                    "hot signer key".to_string(),
                )),
            ))
            .await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::ConfirmXpub),
            ))
            .await;
        sandbox.check(|step| assert!(step.modal.is_none()));

        // Edit sequence
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::RecoveryPath(
                    0,
                    message::DefinePath::SequenceEdited(1000),
                ),
            ))
            .await;

        // Edit recovery key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::RecoveryPath(
                    0,
                    message::DefinePath::Key(0, message::DefineKey::Edit),
                ),
            ))
            .await;
        sandbox.check(|step| assert!(step.modal.is_some()));
        sandbox.update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(
                    message::ImportKeyModal::XPubEdited("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK".to_string()),
                )
        )).await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::NameEdited(
                    "External recovery key".to_string(),
                )),
            ))
            .await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::ConfirmXpub),
            ))
            .await;
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!((step).apply(&mut ctx));
            assert!(ctx
                .descriptor
                .as_ref()
                .unwrap()
                .to_string()
                .contains(&step.signer.lock().unwrap().fingerprint().to_string()));
        });
    }

    #[tokio::test]
    async fn test_define_descriptor_stores_if_hw_is_used() {
        let mut ctx = Context::new(Network::Signet, PathBuf::from_str("/").unwrap());
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(Arc::new(
            Mutex::new(Signer::generate(Network::Bitcoin).unwrap()),
        )));

        let specter_key = message::DefinePath::Key(
            0,
            message::DefineKey::Edited(
                "My Specter key".to_string(),
                DescriptorPublicKey::from_str("[4df3f0e3/84'/0'/0']tpubDDRs9DnRUiJc4hq92PSJKhfzQBgHJUrDo7T2i48smsDfLsQcm3Vh7JhuGqJv8zozVkNFin8YPgpmn2NWNmpRaE3GW2pSxbmAzYf2juy7LeW").unwrap(),
                Some(DeviceKind::Specter),
            ),
        );

        // Use Specter device for primary key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::PrimaryPath(specter_key.clone()),
            ))
            .await;

        // Edit recovery key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::RecoveryPath(
                    0,
                    message::DefinePath::Key(0, message::DefineKey::Edit),
                ),
            ))
            .await;
        sandbox.check(|step| assert!(step.modal.is_some()));
        sandbox.update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(
                    message::ImportKeyModal::XPubEdited("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK".to_string()),
                )
        )).await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::NameEdited(
                    "External recovery key".to_string(),
                )),
            ))
            .await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::ConfirmXpub),
            ))
            .await;
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });

        // Now edit primary key to use hot signer instead of Specter device
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::PrimaryPath(message::DefinePath::Key(
                    0,
                    message::DefineKey::Edit,
                )),
            ))
            .await;
        sandbox.check(|step| assert!(step.modal.is_some()));
        sandbox.update(Message::UseHotSigner).await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::NameEdited(
                    "hot signer key".to_string(),
                )),
            ))
            .await;
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeyModal(message::ImportKeyModal::ConfirmXpub),
            ))
            .await;
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!((step).apply(&mut ctx));
            assert!(!ctx.hw_is_used);
        });

        // Now edit the recovery key to use Specter device
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::RecoveryPath(0, specter_key.clone()),
            ))
            .await;
        sandbox.check(|step| {
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });
    }
}
