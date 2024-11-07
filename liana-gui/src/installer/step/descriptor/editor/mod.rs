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

use crate::installer::context::DescriptorTemplate;
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
    // sequence is 0 if it is a primary path.
    sequence: u16,
    duplicate_sequence: bool,
}

impl Path {
    pub fn new_primary_path() -> Self {
        Self {
            keys: vec![None],
            threshold: 1,
            sequence: 0,
            duplicate_sequence: false,
        }
    }

    pub fn new_recovery_path() -> Self {
        Self {
            keys: vec![None],
            threshold: 1,
            sequence: u16::MAX,
            duplicate_sequence: false,
        }
    }

    pub fn with_n_keys(mut self, n: usize) -> Self {
        self.keys = Vec::new();
        for _i in 0..n {
            self.keys.push(None);
        }
        self
    }

    pub fn with_threshold(mut self, t: usize) -> Self {
        self.threshold = if t > self.keys.len() {
            self.keys.len()
        } else {
            t
        };
        self
    }

    fn valid(&self) -> bool {
        !self.keys.is_empty() && !self.keys.iter().any(|k| k.is_none()) && !self.duplicate_sequence
    }
}

pub struct DefineDescriptor {
    network: Network,
    use_taproot: bool,

    modal: Option<Box<dyn DescriptorEditModal>>,
    signer: Arc<Mutex<Signer>>,
    signer_fingerprint: Fingerprint,

    keys: HashMap<Fingerprint, Key>,
    paths: Vec<Path>,
    descriptor_template: DescriptorTemplate,

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
            keys: HashMap::new(),
            descriptor_template: DescriptorTemplate::default(),
            paths: Vec::new(),
        }
    }

    fn path_keys<'a>(&'a self, p: &Path) -> Vec<Option<&'a Key>> {
        p.keys
            .iter()
            .map(|f| {
                if let Some(f) = f {
                    self.keys.get(f)
                } else {
                    None
                }
            })
            .collect()
    }

    fn check_for_duplicate(&mut self) {
        let mut all_sequence = HashSet::new();
        let mut duplicate_sequences = HashSet::new();
        for path in &mut self.paths {
            if all_sequence.contains(&path.sequence) {
                duplicate_sequences.insert(path.sequence);
            } else {
                all_sequence.insert(path.sequence);
            }
        }

        for path in &mut self.paths {
            path.duplicate_sequence = duplicate_sequences.contains(&path.sequence);
        }
    }

    fn valid(&self) -> bool {
        !self.paths.iter().any(|path| {
            !path.valid()
                || (self.use_taproot
                    && path.keys.iter().any(|k| {
                        if let Some(k) = k.and_then(|k| self.keys.get(&k)) {
                            !k.is_compatible_taproot
                        } else {
                            false
                        }
                    }))
        }) && self.paths.len() >= 2
    }

    fn check_setup(&mut self) {
        self.check_for_duplicate();
    }

    fn load_template(&mut self, template: DescriptorTemplate) {
        if self.descriptor_template != template || self.paths.is_empty() {
            match template {
                DescriptorTemplate::SimpleInheritance => {
                    self.paths = vec![Path::new_primary_path(), Path::new_recovery_path()];
                }
                DescriptorTemplate::MultisigSecurity => {
                    self.paths = vec![
                        Path::new_primary_path().with_n_keys(2).with_threshold(2),
                        Path::new_recovery_path().with_n_keys(3).with_threshold(2),
                    ];
                }
                DescriptorTemplate::Custom => {
                    self.paths = vec![Path::new_primary_path(), Path::new_recovery_path()];
                }
            }
        }
        self.descriptor_template = template;
    }
}

impl Step for DefineDescriptor {
    fn load_context(&mut self, ctx: &Context) {
        self.load_template(ctx.descriptor_template)
    }
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
            Message::DefineDescriptor(message::DefineDescriptor::ChangeTemplate(template)) => {
                self.descriptor_template = template;
            }
            Message::DefineDescriptor(message::DefineDescriptor::AddRecoveryPath) => {
                self.paths.push(Path::new_recovery_path());
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdited(coordinate, key)) => {
                hws.set_alias(key.fingerprint, key.name.clone());
                for (i, j) in coordinate {
                    self.paths[i].keys[j] = Some(key.fingerprint);
                }
                self.keys.insert(key.fingerprint, key);
                self.modal = None;
                self.check_setup();
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(coordinate)) => {
                let use_taproot = self.use_taproot;
                let mut set = HashSet::<Fingerprint>::new();
                let key = coordinate
                    .first()
                    .and_then(|(i, j)| self.paths[*i].keys[*j])
                    .and_then(|f| self.keys.get(&f))
                    .cloned();
                for (i, j) in &coordinate {
                    set.extend(self.paths[*i].keys.iter().filter_map(|key| {
                        if key.is_some() && key != &self.paths[*i].keys[*j] {
                            *key
                        } else {
                            None
                        }
                    }));
                }
                let modal = EditXpubModal::new(
                    use_taproot,
                    set,
                    key,
                    coordinate,
                    self.network,
                    self.signer.clone(),
                    self.signer_fingerprint,
                    self.keys.values().cloned().collect(),
                );
                let cmd = modal.load();
                self.modal = Some(Box::new(modal));
                return cmd;
            }
            Message::DefineDescriptor(message::DefineDescriptor::Path(i, msg)) => match msg {
                message::DefinePath::SequenceEdited(seq) => {
                    self.modal = None;
                    if let Some(path) = self.paths.get_mut(i) {
                        path.sequence = seq;
                    }
                    self.check_for_duplicate();
                }
                message::DefinePath::ThresholdEdited(t) => {
                    self.modal = None;
                    if let Some(path) = self.paths.get_mut(i) {
                        path.threshold = t;
                    }
                }
                message::DefinePath::EditSequence => {
                    if let Some(path) = self.paths.get(i) {
                        self.modal = Some(Box::new(EditSequenceModal::new(i, path.sequence)));
                    }
                }
                message::DefinePath::EditThreshold => {
                    if let Some(path) = self.paths.get(i) {
                        self.modal = Some(Box::new(EditThresholdModal::new(
                            i,
                            (path.threshold, path.keys.len()),
                        )));
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
                            path.keys[j].and_then(|f| self.keys.get(&f)).cloned(),
                            vec![(i, j)],
                            self.network,
                            self.signer.clone(),
                            self.signer_fingerprint,
                            self.keys.values().cloned().collect(),
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
                        // Only delete recovery paths.
                        if i > 0
                            && self
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
                .get(&fingerprint)
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
                    .get(&fingerprint)
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
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let content = match self.descriptor_template {
            DescriptorTemplate::SimpleInheritance => {
                view::editor::template::inheritance::inheritance_template(
                    progress,
                    self.use_taproot,
                    self.paths[0].keys[0]
                        .as_ref()
                        .and_then(|f| self.keys.get(f)),
                    self.paths[1].keys[0]
                        .as_ref()
                        .and_then(|f| self.keys.get(f)),
                    self.paths[1].sequence,
                    self.valid(),
                )
            }
            DescriptorTemplate::MultisigSecurity => {
                view::editor::template::multisig_security_wallet::multisig_security_template(
                    progress,
                    self.use_taproot,
                    self.path_keys(&self.paths[0]),
                    self.path_keys(&self.paths[1]),
                    self.paths[1].sequence,
                    self.paths[1].threshold,
                    self.valid(),
                )
            }
            DescriptorTemplate::Custom => view::editor::template::custom::custom_template(
                progress,
                self.use_taproot,
                view::editor::template::custom::Path {
                    keys: self.path_keys(&self.paths[0]),
                    sequence: self.paths[0].sequence,
                    duplicate_sequence: self.paths[0].duplicate_sequence,
                    threshold: self.paths[0].threshold,
                },
                &mut self.paths[1..]
                    .iter()
                    .map(|p| view::editor::template::custom::Path {
                        sequence: p.sequence,
                        duplicate_sequence: p.duplicate_sequence,
                        threshold: p.threshold,
                        keys: self.path_keys(p),
                    }),
                self.valid(),
            ),
        };
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
        if let Message::DefineDescriptor(message::DefineDescriptor::ThresholdSequenceModal(msg)) =
            message
        {
            match msg {
                message::ThresholdSequenceModal::SequenceEdited(seq) => {
                    if let Ok(s) = u16::from_str(&seq) {
                        self.sequence.valid = s != 0
                    } else {
                        self.sequence.valid = false;
                    }
                    self.sequence.value = seq;
                }
                message::ThresholdSequenceModal::Confirm => {
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
                _ => {}
            }
        }
        Command::none()
    }

    fn view(&self, _hws: &HardwareWallets) -> Element<Message> {
        view::editor::edit_sequence_modal(&self.sequence)
    }
}

pub struct EditThresholdModal {
    threshold: (usize, usize),
    path_index: usize,
}

impl EditThresholdModal {
    pub fn new(path_index: usize, threshold: (usize, usize)) -> Self {
        Self {
            threshold,
            path_index,
        }
    }
}

impl DescriptorEditModal for EditThresholdModal {
    fn processing(&self) -> bool {
        false
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        if let Message::DefineDescriptor(message::DefineDescriptor::ThresholdSequenceModal(msg)) =
            message
        {
            match msg {
                message::ThresholdSequenceModal::ThresholdEdited(threshold) => {
                    if threshold <= self.threshold.1 {
                        self.threshold.0 = threshold;
                    }
                }
                message::ThresholdSequenceModal::Confirm => {
                    let path_index = self.path_index;
                    let threshold = self.threshold.0;
                    return Command::perform(
                        async move { (path_index, threshold) },
                        |(path_index, threshold)| {
                            message::DefineDescriptor::Path(
                                path_index,
                                message::DefinePath::ThresholdEdited(threshold),
                            )
                        },
                    )
                    .map(Message::DefineDescriptor);
                }
                _ => {}
            }
        }
        Command::none()
    }

    fn view(&self, _hws: &HardwareWallets) -> Element<Message> {
        view::editor::edit_threshold_modal(self.threshold)
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
            Network::Signet,
            Arc::new(Mutex::new(Signer::generate(Network::Bitcoin).unwrap())),
        ));
        sandbox.load(&ctx).await;

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

        let key = DescriptorPublicKey::from_str("[4df3f0e3/84'/0'/0']tpubDDRs9DnRUiJc4hq92PSJKhfzQBgHJUrDo7T2i48smsDfLsQcm3Vh7JhuGqJv8zozVkNFin8YPgpmn2NWNmpRaE3GW2pSxbmAzYf2juy7LeW").unwrap();
        let specter_key = Key {
            name: "My Specter key".to_string(),
            fingerprint: key.master_fingerprint(),
            key,
            device_kind: Some(async_hwi::DeviceKind::Specter),
            device_version: None,
            is_compatible_taproot: false,
            is_hot_signer: false,
        };

        // Use Specter device for primary key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeysEdited(vec![(0, 0)], specter_key.clone()),
            ))
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
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeysEdited(vec![(1, 0)], specter_key.clone()),
            ))
            .await;
        sandbox.check(|step| {
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });
    }
}
