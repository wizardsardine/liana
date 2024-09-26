pub mod key;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::iter::FromIterator;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use iced::{Command, Subscription};
use liana::{
    descriptors::{LianaDescriptor, LianaPolicy, PathInfo},
    miniscript::{
        bitcoin::{bip32::Fingerprint, Network},
        descriptor::DescriptorPublicKey,
    },
};

use liana_ui::{
    component::{form, modal::Modal},
    widget::Element,
};

use crate::hw;
use crate::{
    app::settings::KeySetting,
    hw::HardwareWallets,
    installer::{
        message::{self, Message},
        step::{Context, Step},
        view,
    },
    signer::Signer,
};

use key::{check_key_network, new_multixkey_from_xpub, EditXpubModal, Key};

pub trait DescriptorEditModal {
    fn processing(&self) -> bool {
        false
    }
    fn update(&mut self, _hws: &mut HardwareWallets, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn view<'a>(&'a self, _hws: &'a HardwareWallets) -> Element<'a, Message>;
    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        Subscription::none()
    }
}

pub struct RecoveryPath {
    keys: Vec<Option<Fingerprint>>,
    threshold: usize,
    sequence: u16,
    duplicate_sequence: bool,
}

impl RecoveryPath {
    pub fn new() -> Self {
        Self {
            keys: vec![None],
            threshold: 1,
            sequence: u16::MAX,
            duplicate_sequence: false,
        }
    }

    fn valid(&self) -> bool {
        !self.keys.is_empty() && !self.keys.iter().any(|k| k.is_none()) && !self.duplicate_sequence
    }

    fn view(
        &self,
        aliases: &HashMap<Fingerprint, String>,
        duplicate_name: &HashSet<Fingerprint>,
        incompatible_with_tapminiscript: &HashSet<Fingerprint>,
    ) -> Element<message::DefinePath> {
        view::recovery_path_view(
            self.sequence,
            self.duplicate_sequence,
            self.threshold,
            self.keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    if let Some(key) = key {
                        view::defined_descriptor_key(
                            aliases.get(key).unwrap().to_string(),
                            duplicate_name.contains(key),
                            incompatible_with_tapminiscript.contains(key),
                        )
                    } else {
                        view::undefined_descriptor_key()
                    }
                    .map(move |msg| message::DefinePath::Key(i, msg))
                })
                .collect(),
        )
    }
}

struct Setup {
    keys: Vec<Key>,
    duplicate_name: HashSet<Fingerprint>,
    incompatible_with_tapminiscript: HashSet<Fingerprint>,
    spending_keys: Vec<Option<Fingerprint>>,
    spending_threshold: usize,
    recovery_paths: Vec<RecoveryPath>,
}

impl Setup {
    fn new() -> Self {
        Self {
            keys: Vec::new(),
            duplicate_name: HashSet::new(),
            incompatible_with_tapminiscript: HashSet::new(),
            spending_keys: vec![None],
            spending_threshold: 1,
            recovery_paths: vec![RecoveryPath::new()],
        }
    }

    fn valid(&self) -> bool {
        !self.spending_keys.is_empty()
            && !self.spending_keys.iter().any(|k| k.is_none())
            && !self.recovery_paths.iter().any(|path| !path.valid())
            && self.duplicate_name.is_empty()
            && self.incompatible_with_tapminiscript.is_empty()
    }

    // Mark as duplicate every defined key that have the same name but not the same fingerprint.
    // And every undefined_key that have a same name than an other key.
    fn check_for_duplicate(&mut self) {
        self.duplicate_name = HashSet::new();
        for a in &self.keys {
            for b in &self.keys {
                if a.name == b.name && a.fingerprint != b.fingerprint {
                    self.duplicate_name.insert(a.fingerprint);
                    self.duplicate_name.insert(b.fingerprint);
                }
            }
        }

        let mut all_sequence = HashSet::new();
        let mut duplicate_sequence = HashSet::new();
        for path in &mut self.recovery_paths {
            if all_sequence.contains(&path.sequence) {
                duplicate_sequence.insert(path.sequence);
            } else {
                all_sequence.insert(path.sequence);
            }
        }

        for path in &mut self.recovery_paths {
            path.duplicate_sequence = duplicate_sequence.contains(&path.sequence);
        }
    }

    fn check_for_tapminiscript_support(&mut self, must_support_taproot: bool) {
        self.incompatible_with_tapminiscript = HashSet::new();
        if must_support_taproot {
            for key in &self.keys {
                // check if key is used by a path
                if !self
                    .spending_keys
                    .iter()
                    .chain(self.recovery_paths.iter().flat_map(|path| &path.keys))
                    .any(|k| *k == Some(key.fingerprint))
                {
                    continue;
                }

                // device_kind is none only for HotSigner which is compatible.
                if let Some(device_kind) = key.device_kind.as_ref() {
                    if !hw::is_compatible_with_tapminiscript(
                        device_kind,
                        key.device_version.as_ref(),
                    ) {
                        self.incompatible_with_tapminiscript.insert(key.fingerprint);
                    }
                }
            }
        }
    }

    fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for key in &self.keys {
            map.insert(key.key.master_fingerprint(), key.name.clone());
        }
        map
    }
}

pub struct DefineDescriptor {
    setup: Setup,

    network: Network,
    use_taproot: bool,

    modal: Option<Box<dyn DescriptorEditModal>>,
    signer: Arc<Mutex<Signer>>,

    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new(network: Network, signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            network,
            use_taproot: false,
            setup: Setup::new(),
            modal: None,
            signer,
            error: None,
        }
    }

    fn valid(&self) -> bool {
        self.setup.valid()
    }
    fn setup_mut(&mut self) -> &mut Setup {
        &mut self.setup
    }

    fn check_setup(&mut self) {
        self.setup_mut().check_for_duplicate();
        let use_taproot = self.use_taproot;
        self.setup_mut()
            .check_for_tapminiscript_support(use_taproot);
    }
}

impl Step for DefineDescriptor {
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        self.error = None;
        match message {
            Message::Close => {
                self.modal = None;
            }
            Message::CreateTaprootDescriptor(use_taproot) => {
                self.use_taproot = use_taproot;
                self.check_setup();
            }
            Message::DefineDescriptor(message::DefineDescriptor::AddRecoveryPath) => {
                self.setup_mut().recovery_paths.push(RecoveryPath::new());
            }
            Message::DefineDescriptor(message::DefineDescriptor::PrimaryPath(msg)) => match msg {
                message::DefinePath::ThresholdEdited(value) => {
                    self.setup_mut().spending_threshold = value;
                }
                message::DefinePath::AddKey => {
                    self.setup_mut().spending_keys.push(None);
                    self.setup_mut().spending_threshold += 1;
                }
                message::DefinePath::Key(i, msg) => match msg {
                    message::DefineKey::Clipboard(key) => {
                        return Command::perform(async move { key }, Message::Clibpboard);
                    }
                    message::DefineKey::Edited(name, imported_key, kind, version) => {
                        let fingerprint = imported_key.master_fingerprint();
                        hws.set_alias(fingerprint, name.clone());
                        if let Some(key) = self
                            .setup_mut()
                            .keys
                            .iter_mut()
                            .find(|k| k.fingerprint == fingerprint)
                        {
                            key.name = name;
                        } else {
                            self.setup_mut().keys.push(Key {
                                fingerprint,
                                name,
                                key: imported_key,
                                device_kind: kind,
                                device_version: version,
                            });
                        }

                        self.setup_mut().spending_keys[i] = Some(fingerprint);

                        self.modal = None;
                        self.check_setup();
                    }
                    message::DefineKey::Edit => {
                        let use_taproot = self.use_taproot;
                        let network = self.network;
                        let setup = self.setup_mut();
                        let modal = EditXpubModal::new(
                            use_taproot,
                            HashSet::from_iter(setup.spending_keys.iter().filter_map(|key| {
                                if key.is_some() && key != &setup.spending_keys[i] {
                                    *key
                                } else {
                                    None
                                }
                            })),
                            self.setup_mut().spending_keys[i],
                            None,
                            i,
                            network,
                            self.signer.clone(),
                            self.setup_mut()
                                .keys
                                .iter()
                                .filter(|k| check_key_network(&k.key, network))
                                .cloned()
                                .collect(),
                        );
                        let cmd = modal.load();
                        self.modal = Some(Box::new(modal));
                        return cmd;
                    }
                    message::DefineKey::Delete => {
                        self.setup_mut().spending_keys.remove(i);
                        if self.setup_mut().spending_threshold
                            > self.setup_mut().spending_keys.len()
                        {
                            self.setup_mut().spending_threshold -= 1;
                        }
                        self.check_setup();
                    }
                },
                _ => {}
            },
            Message::DefineDescriptor(message::DefineDescriptor::RecoveryPath(i, msg)) => match msg
            {
                message::DefinePath::ThresholdEdited(value) => {
                    if let Some(path) = self.setup_mut().recovery_paths.get_mut(i) {
                        path.threshold = value;
                    }
                }
                message::DefinePath::SequenceEdited(seq) => {
                    self.modal = None;
                    if let Some(path) = self.setup_mut().recovery_paths.get_mut(i) {
                        path.sequence = seq;
                    }
                    self.setup_mut().check_for_duplicate();
                }
                message::DefinePath::EditSequence => {
                    if let Some(path) = self.setup_mut().recovery_paths.get(i) {
                        self.modal = Some(Box::new(EditSequenceModal::new(i, path.sequence)));
                    }
                }
                message::DefinePath::AddKey => {
                    if let Some(path) = self.setup_mut().recovery_paths.get_mut(i) {
                        path.keys.push(None);
                        path.threshold += 1;
                    }
                }
                message::DefinePath::Key(j, msg) => match msg {
                    message::DefineKey::Clipboard(key) => {
                        return Command::perform(async move { key }, Message::Clibpboard);
                    }
                    message::DefineKey::Edited(name, imported_key, kind, version) => {
                        let fingerprint = imported_key.master_fingerprint();
                        hws.set_alias(fingerprint, name.clone());
                        if let Some(key) = self
                            .setup_mut()
                            .keys
                            .iter_mut()
                            .find(|k| k.fingerprint == fingerprint)
                        {
                            key.name = name;
                        } else {
                            self.setup_mut().keys.push(Key {
                                fingerprint,
                                name,
                                key: imported_key,
                                device_kind: kind,
                                device_version: version,
                            });
                        }

                        self.setup_mut().recovery_paths[i].keys[j] = Some(fingerprint);

                        self.modal = None;
                        self.check_setup();
                    }
                    message::DefineKey::Edit => {
                        let use_taproot = self.use_taproot;
                        let setup = self.setup_mut();
                        let modal = EditXpubModal::new(
                            use_taproot,
                            HashSet::from_iter(setup.recovery_paths[i].keys.iter().filter_map(
                                |key| {
                                    if key.is_some() && key != &setup.recovery_paths[i].keys[j] {
                                        *key
                                    } else {
                                        None
                                    }
                                },
                            )),
                            setup.recovery_paths[i].keys[j],
                            Some(i),
                            j,
                            self.network,
                            self.signer.clone(),
                            self.setup.keys.clone(),
                        );
                        let cmd = modal.load();
                        self.modal = Some(Box::new(modal));
                        return cmd;
                    }
                    message::DefineKey::Delete => {
                        if let Some(path) = self.setup_mut().recovery_paths.get_mut(i) {
                            path.keys.remove(j);
                            if path.threshold > path.keys.len() {
                                path.threshold -= 1;
                            }
                        }
                        if self
                            .setup_mut()
                            .recovery_paths
                            .get(i)
                            .map(|path| path.keys.is_empty())
                            .unwrap_or(false)
                        {
                            self.setup_mut().recovery_paths.remove(i);
                        }
                        self.check_setup();
                    }
                },
            },
            _ => {
                if let Some(modal) = &mut self.modal {
                    return modal.update(hws, message);
                }
            }
        };
        Command::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        if let Some(modal) = &self.modal {
            modal.subscription(hws)
        } else {
            Subscription::none()
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        ctx.bitcoin_config.network = self.network;
        ctx.keys = Vec::new();
        let mut hw_is_used = false;
        let mut spending_keys: Vec<DescriptorPublicKey> = Vec::new();
        let mut key_derivation_index = HashMap::<Fingerprint, usize>::new();
        for spending_key in self.setup.spending_keys.iter().clone() {
            let fingerprint = spending_key.expect("Must be present at this step");
            let key = self
                .setup
                .keys
                .iter()
                .find(|key| key.key.master_fingerprint() == fingerprint)
                .expect("Must be present at this step");
            if let DescriptorPublicKey::XPub(xpub) = &key.key {
                if let Some((master_fingerprint, _)) = xpub.origin {
                    ctx.keys.push(KeySetting {
                        master_fingerprint,
                        name: key.name.clone(),
                    });
                    if key.device_kind.is_some() {
                        hw_is_used = true;
                    }
                }
                let derivation_index = key_derivation_index.get(&fingerprint).unwrap_or(&0);
                spending_keys.push(DescriptorPublicKey::MultiXPub(new_multixkey_from_xpub(
                    xpub.clone(),
                    *derivation_index,
                )));
                key_derivation_index.insert(fingerprint, derivation_index + 1);
            }
        }

        let mut recovery_paths = BTreeMap::new();

        for path in &self.setup.recovery_paths {
            let mut recovery_keys: Vec<DescriptorPublicKey> = Vec::new();
            for recovery_key in path.keys.iter().clone() {
                let fingerprint = recovery_key.expect("Must be present at this step");
                let key = self
                    .setup
                    .keys
                    .iter()
                    .find(|key| key.key.master_fingerprint() == fingerprint)
                    .expect("Must be present at this step");
                if let DescriptorPublicKey::XPub(xpub) = &key.key {
                    if let Some((master_fingerprint, _)) = xpub.origin {
                        ctx.keys.push(KeySetting {
                            master_fingerprint,
                            name: key.name.clone(),
                        });
                        if key.device_kind.is_some() {
                            hw_is_used = true;
                        }
                    }

                    let derivation_index = key_derivation_index.get(&fingerprint).unwrap_or(&0);
                    recovery_keys.push(DescriptorPublicKey::MultiXPub(new_multixkey_from_xpub(
                        xpub.clone(),
                        *derivation_index,
                    )));
                    key_derivation_index.insert(fingerprint, derivation_index + 1);
                }
            }

            let recovery_keys = if recovery_keys.len() == 1 {
                PathInfo::Single(recovery_keys[0].clone())
            } else {
                PathInfo::Multi(path.threshold, recovery_keys)
            };

            recovery_paths.insert(path.sequence, recovery_keys);
        }

        if spending_keys.is_empty() {
            return false;
        }

        let spending_keys = if spending_keys.len() == 1 {
            PathInfo::Single(spending_keys[0].clone())
        } else {
            PathInfo::Multi(self.setup.spending_threshold, spending_keys)
        };

        let policy = match if self.use_taproot {
            LianaPolicy::new(spending_keys, recovery_paths)
        } else {
            LianaPolicy::new_legacy(spending_keys, recovery_paths)
        } {
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

    fn view<'a>(
        &'a self,
        hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let aliases = self.setup.keys_aliases();
        let content = view::define_descriptor(
            progress,
            email,
            self.use_taproot,
            self.setup
                .spending_keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    if let Some(key) = key {
                        view::defined_descriptor_key(
                            aliases.get(key).unwrap().to_string(),
                            self.setup.duplicate_name.contains(key),
                            self.setup.incompatible_with_tapminiscript.contains(key),
                        )
                    } else {
                        view::undefined_descriptor_key()
                    }
                    .map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::PrimaryPath(
                            message::DefinePath::Key(i, msg),
                        ))
                    })
                })
                .collect(),
            self.setup.spending_threshold,
            self.setup
                .recovery_paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    path.view(
                        &aliases,
                        &self.setup.duplicate_name,
                        &self.setup.incompatible_with_tapminiscript,
                    )
                    .map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::RecoveryPath(i, msg))
                    })
                })
                .collect(),
            self.valid(),
            self.error.as_ref(),
        );
        if let Some(modal) = &self.modal {
            Modal::new(content, modal.view(hws))
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

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
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

    fn view(&self, _hws: &HardwareWallets) -> Element<Message> {
        view::edit_sequence_modal(&self.sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_runtime::command::Action;
    use std::path::PathBuf;
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
            let mut hws = HardwareWallets::new(PathBuf::from_str("/").unwrap(), Network::Bitcoin);
            let cmd = self.step.lock().unwrap().update(&mut hws, message);
            for action in cmd.actions() {
                if let Action::Future(f) = action {
                    let msg = f.await;
                    let _cmd = self.step.lock().unwrap().update(&mut hws, msg);
                }
            }
        }
        pub async fn load(&self, ctx: &Context) {
            self.step.lock().unwrap().load_context(ctx);
        }
    }

    #[tokio::test]
    async fn test_define_descriptor_use_hotkey() {
        let mut ctx = Context::new(
            Network::Signet,
            PathBuf::from_str("/").unwrap(),
            crate::installer::context::RemoteBackend::None,
        );
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(
            Network::Bitcoin,
            Arc::new(Mutex::new(Signer::generate(Network::Bitcoin).unwrap())),
        ));

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
        let mut ctx = Context::new(
            Network::Testnet,
            PathBuf::from_str("/").unwrap(),
            crate::installer::context::RemoteBackend::None,
        );
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(
            Network::Testnet,
            Arc::new(Mutex::new(Signer::generate(Network::Testnet).unwrap())),
        ));
        sandbox.load(&ctx).await;

        let specter_key = message::DefinePath::Key(
            0,
            message::DefineKey::Edited(
                "My Specter key".to_string(),
                DescriptorPublicKey::from_str("[4df3f0e3/84'/0'/0']tpubDDRs9DnRUiJc4hq92PSJKhfzQBgHJUrDo7T2i48smsDfLsQcm3Vh7JhuGqJv8zozVkNFin8YPgpmn2NWNmpRaE3GW2pSxbmAzYf2juy7LeW").unwrap(),
                Some(async_hwi::DeviceKind::Specter),
                None,
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
