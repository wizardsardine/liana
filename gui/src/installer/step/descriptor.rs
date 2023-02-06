use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

use iced::{Command, Element};
use liana::{
    descriptors::{LianaDescKeys, MultipathDescriptor},
    miniscript::{
        bitcoin::{
            util::bip32::{ChildNumber, DerivationPath, Fingerprint},
            Network,
        },
        descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
    },
};

use async_hwi::DeviceKind;

use crate::{
    app::settings::KeySetting,
    hw::{list_hardware_wallets, HardwareWallet},
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view, Error,
    },
    ui::component::{form, modal::Modal},
};

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
            spending_keys: vec![DescriptorKey::default()],
            spending_threshold: 1,
            recovery_keys: vec![DescriptorKey::default()],
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
    // Mark as duplicate every defined key that have the same name but not the same fingerprint.
    // And every undefined_key that have a same name than an other key.
    fn check_for_duplicate(&mut self) {
        let mut all_keys = HashSet::new();
        let mut duplicate_keys = HashSet::new();
        let mut all_names: HashMap<String, Fingerprint> = HashMap::new();
        let mut duplicate_names = HashSet::new();
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
        for recovery_key in &self.recovery_keys {
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
        for spending_key in self.spending_keys.iter_mut() {
            spending_key.duplicate_name = duplicate_names.contains(&spending_key.name);
            if let Some(key) = &spending_key.key {
                spending_key.duplicate_key = duplicate_keys.contains(key);
            }
        }
        for recovery_key in self.recovery_keys.iter_mut() {
            if let Some(key) = &recovery_key.key {
                recovery_key.duplicate_key = duplicate_keys.contains(key);
            }
        }
    }

    fn edit_alias_for_key_with_same_fingerprint(&mut self, name: String, fingerprint: Fingerprint) {
        for spending_key in &mut self.spending_keys {
            if spending_key.key.as_ref().map(|k| k.master_fingerprint()) == Some(fingerprint) {
                spending_key.name = name.clone();
            }
        }
        for recovery_key in &mut self.recovery_keys {
            if recovery_key.key.as_ref().map(|k| k.master_fingerprint()) == Some(fingerprint) {
                recovery_key.name = name.clone();
            }
        }
    }

    /// Returns the maximum account index per key fingerprint
    fn fingerprint_account_index_mappping(&self) -> HashMap<Fingerprint, ChildNumber> {
        let mut mapping = HashMap::new();
        let update_mapping =
            |keys: &[DescriptorKey], mapping: &mut HashMap<Fingerprint, ChildNumber>| {
                for key in keys {
                    if let Some(DescriptorPublicKey::MultiXPub(key)) = key.key.as_ref() {
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
        update_mapping(&self.recovery_keys, &mut mapping);
        mapping
    }

    fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for spending_key in &self.spending_keys {
            if let Some(key) = spending_key.key.as_ref() {
                map.insert(key.master_fingerprint(), spending_key.name.clone());
            }
        }
        for recovery_key in &self.recovery_keys {
            if let Some(key) = recovery_key.key.as_ref() {
                map.insert(key.master_fingerprint(), recovery_key.name.clone());
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
                            self.recovery_keys.push(DescriptorKey::default());
                            self.recovery_threshold += 1;
                        } else {
                            self.spending_keys.push(DescriptorKey::default());
                            self.spending_threshold += 1;
                        }
                    }
                    message::DefineDescriptor::Key(is_recovery, i, msg) => match msg {
                        message::DefineKey::Clipboard(key) => {
                            return Command::perform(async move { key }, Message::Clibpboard);
                        }
                        message::DefineKey::Edited(name, imported_key) => {
                            self.edit_alias_for_key_with_same_fingerprint(
                                name.clone(),
                                imported_key.master_fingerprint(),
                            );
                            if is_recovery {
                                if let Some(recovery_key) = self.recovery_keys.get_mut(i) {
                                    recovery_key.name = name;
                                    recovery_key.key = Some(imported_key);
                                    recovery_key.check_network(self.network);
                                }
                            } else if let Some(spending_key) = self.spending_keys.get_mut(i) {
                                spending_key.name = name;
                                spending_key.key = Some(imported_key);
                                spending_key.check_network(self.network);
                            }
                            self.modal = None;
                            self.check_for_duplicate();
                        }
                        message::DefineKey::Edit => {
                            if is_recovery {
                                if let Some(recovery_key) = self.recovery_keys.get(i) {
                                    let name = recovery_key.name.clone();
                                    let key = recovery_key
                                        .key
                                        .as_ref()
                                        .map(|k| {
                                            k.to_string().trim_end_matches("/<0;1>/*").to_string()
                                        })
                                        .unwrap_or_else(|| "".to_string());
                                    let modal = EditXpubModal::new(
                                        name,
                                        key,
                                        i,
                                        is_recovery,
                                        self.network,
                                        self.fingerprint_account_index_mappping(),
                                        self.keys_aliases(),
                                    );
                                    let cmd = modal.load();
                                    self.modal = Some(Box::new(modal));
                                    return cmd;
                                }
                            } else if let Some(spending_key) = self.spending_keys.get(i) {
                                let name = spending_key.name.clone();
                                let key = spending_key
                                    .key
                                    .as_ref()
                                    .map(|k| k.to_string().trim_end_matches("/<0;1>/*").to_string())
                                    .unwrap_or_else(|| "".to_string());
                                let modal = EditXpubModal::new(
                                    name,
                                    key,
                                    i,
                                    is_recovery,
                                    self.network,
                                    self.fingerprint_account_index_mappping(),
                                    self.keys_aliases(),
                                );
                                let cmd = modal.load();
                                self.modal = Some(Box::new(modal));
                                return cmd;
                            }
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
        ctx.keys = Vec::new();
        let mut spending_keys: Vec<DescriptorPublicKey> = Vec::new();
        for spending_key in self.spending_keys.iter().clone() {
            if let Some(key) = spending_key.key.as_ref() {
                if let DescriptorPublicKey::MultiXPub(xpub) = key {
                    if let Some((master_fingerprint, _)) = xpub.origin {
                        ctx.keys.push(KeySetting {
                            master_fingerprint,
                            name: spending_key.name.clone(),
                        });
                    }
                }
                spending_keys.push(key.clone());
            }
        }

        let mut recovery_keys: Vec<DescriptorPublicKey> = Vec::new();
        for recovery_key in self.recovery_keys.iter().clone() {
            if let Some(key) = recovery_key.key.as_ref() {
                if let DescriptorPublicKey::MultiXPub(xpub) = key {
                    if let Some((master_fingerprint, _)) = xpub.origin {
                        ctx.keys.push(KeySetting {
                            master_fingerprint,
                            name: recovery_key.name.clone(),
                        });
                    }
                }
                recovery_keys.push(key.clone());
            }
        }

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

impl Default for DescriptorKey {
    fn default() -> Self {
        Self {
            name: "".to_string(),
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

pub struct EditXpubModal {
    is_recovery: bool,
    key_index: usize,
    network: Network,
    error: Option<Error>,
    processing: bool,

    keys_aliases: HashMap<Fingerprint, String>,
    account_indexes: HashMap<Fingerprint, ChildNumber>,

    form_name: form::Value<String>,
    form_xpub: form::Value<String>,
    edit_name: bool,

    chosen_hw: Option<usize>,
    hws: Vec<HardwareWallet>,
}

impl EditXpubModal {
    fn new(
        name: String,
        key: String,
        key_index: usize,
        is_recovery: bool,
        network: Network,
        account_indexes: HashMap<Fingerprint, ChildNumber>,
        keys_aliases: HashMap<Fingerprint, String>,
    ) -> Self {
        Self {
            form_name: form::Value {
                valid: true,
                value: name,
            },
            form_xpub: form::Value {
                valid: true,
                value: key,
            },
            keys_aliases,
            account_indexes,
            is_recovery,
            key_index,
            chosen_hw: None,
            processing: false,
            hws: Vec::new(),
            error: None,
            network,
            edit_name: false,
        }
    }
    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
            Message::ConnectedHardwareWallets,
        )
    }
}

impl DescriptorKeyModal for EditXpubModal {
    fn processing(&self) -> bool {
        self.processing
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
                    self.chosen_hw = Some(i);
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
                            self.network,
                            account_index,
                        ),
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
                self.hws = Vec::new();
                return self.load();
            }
            Message::DefineDescriptor(message::DefineDescriptor::HWXpubImported(res)) => {
                self.processing = false;
                match res {
                    Ok(key) => {
                        if let Some(alias) = self.keys_aliases.get(&key.master_fingerprint()) {
                            self.form_name.valid = true;
                            self.form_name.value = alias.clone();
                            self.edit_name = false;
                        } else {
                            self.edit_name = true;
                        }
                        self.form_xpub.valid = true;
                        self.form_xpub.value =
                            key.to_string().trim_end_matches("/<0;1>/*").to_string();
                    }
                    Err(e) => {
                        self.error = Some(e);
                    }
                }
            }
            Message::DefineDescriptor(message::DefineDescriptor::EditName) => {
                self.edit_name = true;
            }
            Message::DefineDescriptor(message::DefineDescriptor::NameEdited(name)) => {
                self.form_name.valid = true;
                self.form_name.value = name;
            }
            Message::DefineDescriptor(message::DefineDescriptor::XPubEdited(s)) => {
                if let Ok(DescriptorPublicKey::MultiXPub(key)) =
                    DescriptorPublicKey::from_str(&format!("{}/<0;1>/*", s))
                {
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
            Message::DefineDescriptor(message::DefineDescriptor::ConfirmXpub) => {
                if let Ok(key) =
                    DescriptorPublicKey::from_str(&format!("{}/<0;1>/*", self.form_xpub.value))
                {
                    let key_index = self.key_index;
                    let is_recovery = self.is_recovery;
                    let name = self.form_name.value.clone();
                    return Command::perform(
                        async move { (is_recovery, key_index, key) },
                        |(is_recovery, key_index, key)| {
                            message::DefineDescriptor::Key(
                                is_recovery,
                                key_index,
                                message::DefineKey::Edited(name, key),
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
        view::edit_key_modal(
            self.network,
            &self.hws,
            self.error.as_ref(),
            self.processing,
            self.chosen_hw,
            &self.form_xpub,
            &self.form_name,
            self.edit_name,
        )
    }
}

/// LIANA_STANDARD_PATH: m/48'/0'/0'/2';
/// LIANA_TESTNET_STANDARD_PATH: m/48'/1'/0'/2';
async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    network: Network,
    account_index: ChildNumber,
) -> Result<DescriptorPublicKey, Error> {
    let derivation_path = DerivationPath::from_str(&{
        if network == Network::Bitcoin {
            format!("m/48'/0'/{}/2'", account_index)
        } else {
            format!("m/48'/1'/{}/2'", account_index)
        }
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
                self.xpubs
                    .push(xpub.to_string().trim_end_matches("/<0;1>/*").to_string());
            }
        }
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
            let next_account = self.next_account;
            Command::perform(
                async move {
                    (
                        i,
                        get_extended_pubkey(device, fingerprint, network, next_account).await,
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

pub struct ParticipateXpub {
    network: Network,
    network_valid: bool,
    data_dir: Option<PathBuf>,

    shared: bool,

    xpubs_hw: Vec<HardwareWalletXpubs>,
}

impl ParticipateXpub {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            network_valid: true,
            data_dir: None,
            xpubs_hw: Vec::new(),
            shared: false,
        }
    }
}

impl Step for ParticipateXpub {
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
            Message::UserActionDone(shared) => self.shared = shared,
            Message::ImportXpub(i, res) => {
                if let Some(hw) = self.xpubs_hw.get_mut(i) {
                    hw.update(res);
                }
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
        self.network = ctx.bitcoin_config.network;
        self.data_dir = Some(ctx.data_dir.clone());
        let mut network_datadir = ctx.data_dir.clone();
        network_datadir.push(self.network.to_string());
        self.network_valid = !network_datadir.exists();
    }

    fn load(&self) -> Command<Message> {
        Command::perform(
            list_hardware_wallets(&[], None),
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
            self.shared,
        )
    }
}

impl Default for ParticipateXpub {
    fn default() -> Self {
        Self::new()
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
    descriptor: Option<MultipathDescriptor>,
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
    fn apply(&mut self, ctx: &mut Context) -> bool {
        for (fingerprint, kind, token) in &self.hmacs {
            ctx.hws.push((*kind, *fingerprint, *token));
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
    descriptor: Option<MultipathDescriptor>,
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
