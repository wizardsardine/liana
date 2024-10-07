pub mod key;
pub mod template;

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

use key::{new_multixkey_from_xpub, EditXpubModal, Key};

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

pub struct Path {
    keys: Vec<Option<Fingerprint>>,
    threshold: usize,
    sequence: Option<u16>,
    duplicate_sequence: bool,
}

impl Path {
    pub fn new_primary_path() -> Self {
        Self {
            keys: vec![None],
            threshold: 1,
            sequence: None,
            duplicate_sequence: false,
        }
    }

    pub fn new_recovery_path() -> Self {
        Self {
            keys: vec![None],
            threshold: 1,
            sequence: Some(u16::MAX),
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
        if let Some(sequence) = self.sequence {
            view::editor::recovery_path_view(
                sequence,
                self.duplicate_sequence,
                self.threshold,
                self.keys
                    .iter()
                    .enumerate()
                    .map(|(i, key)| {
                        if let Some(key) = key {
                            view::editor::defined_descriptor_key(
                                aliases.get(key).unwrap().to_string(),
                                duplicate_name.contains(key),
                                incompatible_with_tapminiscript.contains(key),
                            )
                        } else {
                            view::editor::undefined_descriptor_key()
                        }
                        .map(move |msg| message::DefinePath::Key(i, msg))
                    })
                    .collect(),
            )
        } else {
            view::editor::primary_path_view(
                self.threshold,
                self.keys
                    .iter()
                    .enumerate()
                    .map(|(i, key)| {
                        if let Some(key) = key {
                            view::editor::defined_descriptor_key(
                                aliases.get(key).unwrap().to_string(),
                                duplicate_name.contains(key),
                                incompatible_with_tapminiscript.contains(key),
                            )
                        } else {
                            view::editor::undefined_descriptor_key()
                        }
                        .map(move |msg| message::DefinePath::Key(i, msg))
                    })
                    .collect(),
            )
        }
    }
}

pub struct DefineDescriptor {
    network: Network,
    use_taproot: bool,

    modal: Option<Box<dyn DescriptorEditModal>>,
    signer: Arc<Mutex<Signer>>,
    signer_fingerprint: Fingerprint,

    keys: Vec<Key>,
    duplicate_name: HashSet<Fingerprint>,
    incompatible_with_tapminiscript: HashSet<Fingerprint>,
    paths: Vec<Path>,

    error: Option<String>,
}

impl DefineDescriptor {
    pub fn new(network: Network, signer: Arc<Mutex<Signer>>) -> Self {
        let signer_fingerprint = signer.lock().unwrap().fingerprint();
        Self {
            network,
            use_taproot: false,
            modal: None,
            signer_fingerprint,

            signer,
            error: None,
            keys: Vec::new(),
            duplicate_name: HashSet::new(),
            incompatible_with_tapminiscript: HashSet::new(),
            paths: vec![Path::new_primary_path(), Path::new_recovery_path()],
        }
    }

    fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for key in &self.keys {
            map.insert(key.key.master_fingerprint(), key.name.clone());
        }
        map
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
        let mut duplicate_sequences = HashSet::new();
        for path in &mut self.paths {
            if let Some(sequence) = path.sequence {
                if all_sequence.contains(&sequence) {
                    duplicate_sequences.insert(sequence);
                } else {
                    all_sequence.insert(sequence);
                }
            }
        }

        for path in &mut self.paths {
            if let Some(sequence) = path.sequence {
                path.duplicate_sequence = duplicate_sequences.contains(&sequence);
            }
        }
    }

    fn check_for_tapminiscript_support(&mut self, must_support_taproot: bool) {
        self.incompatible_with_tapminiscript = HashSet::new();
        if must_support_taproot {
            for key in &self.keys {
                // check if key is used by a path
                if !self
                    .paths
                    .iter()
                    .flat_map(|path| &path.keys)
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

    fn valid(&self) -> bool {
        !self.paths.iter().any(|path| !path.valid())
            && self.duplicate_name.is_empty()
            && self.incompatible_with_tapminiscript.is_empty()
            && self.paths.len() >= 2
    }

    fn check_setup(&mut self) {
        self.check_for_duplicate();
        let use_taproot = self.use_taproot;
        self.check_for_tapminiscript_support(use_taproot);
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
                self.paths.push(Path::new_recovery_path());
            }
            Message::DefineDescriptor(message::DefineDescriptor::Path(i, msg)) => match msg {
                message::DefinePath::ThresholdEdited(value) => {
                    if let Some(path) = self.paths.get_mut(i) {
                        path.threshold = value;
                    }
                }
                message::DefinePath::SequenceEdited(seq) => {
                    self.modal = None;
                    if let Some(path) = self.paths.get_mut(i) {
                        path.sequence = Some(seq);
                    }
                    self.check_for_duplicate();
                }
                message::DefinePath::EditSequence => {
                    if let Some(path) = self.paths.get(i) {
                        if let Some(sequence) = path.sequence {
                            self.modal = Some(Box::new(EditSequenceModal::new(i, sequence)));
                        }
                    }
                }
                message::DefinePath::AddKey => {
                    if let Some(path) = self.paths.get_mut(i) {
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
                        let is_hot_signer = self.signer_fingerprint == fingerprint;
                        hws.set_alias(fingerprint, name.clone());
                        if let Some(key) =
                            self.keys.iter_mut().find(|k| k.fingerprint == fingerprint)
                        {
                            key.name = name;
                            key.is_hot_signer = is_hot_signer;
                            key.device_kind = kind;
                            key.device_version = version;
                        } else {
                            self.keys.push(Key {
                                fingerprint,
                                is_hot_signer,
                                name,
                                key: imported_key,
                                device_kind: kind,
                                device_version: version,
                            });
                        }

                        self.paths[i].keys[j] = Some(fingerprint);

                        self.modal = None;
                        self.check_setup();
                    }
                    message::DefineKey::Edit => {
                        let use_taproot = self.use_taproot;
                        let path = &self.paths[i];
                        let modal = EditXpubModal::new(
                            use_taproot,
                            HashSet::from_iter(path.keys.iter().filter_map(|key| {
                                if key.is_some() && key != &path.keys[j] {
                                    *key
                                } else {
                                    None
                                }
                            })),
                            path.keys[j],
                            i,
                            j,
                            self.network,
                            self.signer.clone(),
                            self.signer_fingerprint,
                            self.keys.clone(),
                        );
                        let cmd = modal.load();
                        self.modal = Some(Box::new(modal));
                        return cmd;
                    }
                    message::DefineKey::Delete => {
                        if let Some(path) = self.paths.get_mut(i) {
                            path.keys.remove(j);
                            if path.threshold > path.keys.len() {
                                path.threshold -= 1;
                            }
                        }
                        if self
                            .paths
                            .get(i)
                            .map(|path| path.keys.is_empty())
                            .unwrap_or(false)
                        {
                            self.paths.remove(i);
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
        if self.paths.len() < 2 {
            return false;
        }

        ctx.bitcoin_config.network = self.network;
        ctx.keys = Vec::new();
        let mut hw_is_used = false;
        let mut spending_keys: Vec<DescriptorPublicKey> = Vec::new();
        let mut key_derivation_index = HashMap::<Fingerprint, usize>::new();
        for spending_key in self.paths[0].keys.iter().clone() {
            let fingerprint = spending_key.expect("Must be present at this step");
            let key = self
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

        for path in &self.paths[1..] {
            let mut recovery_keys: Vec<DescriptorPublicKey> = Vec::new();
            for recovery_key in path.keys.iter().clone() {
                let fingerprint = recovery_key.expect("Must be present at this step");
                let key = self
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

            recovery_paths.insert(
                path.sequence
                    .expect("Must be a recovery path with a sequence"),
                recovery_keys,
            );
        }

        if spending_keys.is_empty() {
            return false;
        }

        let spending_keys = if spending_keys.len() == 1 {
            PathInfo::Single(spending_keys[0].clone())
        } else {
            PathInfo::Multi(self.paths[0].threshold, spending_keys)
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
        let aliases = self.keys_aliases();
        let content = view::editor::define_descriptor(
            progress,
            email,
            self.use_taproot,
            self.paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    path.view(
                        &aliases,
                        &self.duplicate_name,
                        &self.incompatible_with_tapminiscript,
                    )
                    .map(move |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::Path(i, msg))
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
                                    message::DefineDescriptor::Path(
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
        view::editor::edit_sequence_modal(&self.sequence)
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
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                0,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
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
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                message::DefinePath::SequenceEdited(1000),
            )))
            .await;

        // Edit recovery key
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
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
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                0,
                specter_key.clone(),
            )))
            .await;

        // Edit recovery key
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
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
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                0,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
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
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                specter_key.clone(),
            )))
            .await;
        sandbox.check(|step| {
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });
    }
}
