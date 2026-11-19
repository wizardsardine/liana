pub mod border_wallet_wizard;
pub mod key;
pub mod template;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use coincube_core::miniscript::bitcoin::bip32::ChildNumber;
use coincube_core::{
    descriptors::{CoincubeDescriptor, CoincubePolicy, PathInfo},
    miniscript::{
        bitcoin::{bip32::Fingerprint, Network},
        descriptor::DescriptorPublicKey,
    },
};
use iced::{Subscription, Task};

use coincube_ui::{
    component::form,
    widget::{modal::Modal, Element},
};

use crate::{
    app::settings::KeySetting,
    hw::HardwareWallets,
    installer::{
        context::DescriptorTemplate,
        descriptor::{Key, Path, PathKind, PathSequence, PathWarning},
        message::{self, Message},
        step::{Context, Step},
        view,
    },
    signer::Signer,
};

use key::{new_multixkey_from_xpub, EditKeyAlias, PathData, SelectKeySource};

pub trait DescriptorEditModal {
    fn processing(&self) -> bool {
        false
    }
    fn update(&mut self, _hws: &mut HardwareWallets, _message: Message) -> Task<Message> {
        Task::none()
    }
    fn view<'a>(&'a self, _hws: &'a HardwareWallets) -> Element<'a, Message>;
    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        Subscription::none()
    }
}

pub struct DefineDescriptor {
    network: Network,
    /// The chain the Vault is being created on, from the installer context.
    /// Only read to decide whether the replay-capability notice applies
    /// (Bitcoin Blake2b); `network` stays the encoding for key work.
    chain: crate::chain::ChainId,
    use_taproot: bool,

    modal: Option<Box<dyn DescriptorEditModal>>,
    signer: Arc<Mutex<Signer>>,

    keys: HashMap<Fingerprint, Key>,
    paths: Vec<Path>,
    accounts: HashMap<Fingerprint, ChildNumber>,
    descriptor_template: DescriptorTemplate,

    /// Cube UUID for fetching Keychain keys from the API.
    /// Present when the Vault installer was launched from inside a Cube.
    cube_id: Option<String>,
    /// Authenticated coincube-api client for fetching Keychain keys.
    coincube_client: Option<crate::services::coincube::CoincubeClient>,
    /// This Cube's Connect-blinding encryption key, for opening the xpub
    /// envelopes Connect serves in place of plaintext keys (PR D3).
    cube_encryption_key:
        Option<std::sync::Arc<crate::services::connect::crypto::CubeEncryptionKey>>,

    error: Option<String>,
}

impl DefineDescriptor {
    // Formerly took a `developer_mode` flag that gated the
    // "Generate hot key" button in the old Other-options list. The
    // card-grid redesign promotes that flow to a first-class "Cube
    // Key" card, so the gate is gone.
    pub fn new(network: Network, signer: Arc<Mutex<Signer>>) -> Self {
        Self {
            network,
            chain: crate::chain::ChainId::from(network),
            use_taproot: false,
            modal: None,

            signer,
            error: None,
            keys: HashMap::new(),
            descriptor_template: DescriptorTemplate::default(),
            paths: Vec::new(),
            accounts: Default::default(),
            cube_id: None,
            coincube_client: None,
            cube_encryption_key: None,
        }
    }

    fn check_for_warning(&mut self) {
        let mut all_sequence = HashSet::new();
        let mut duplicate_sequences = HashSet::new();
        for path in &mut self.paths {
            if all_sequence.contains(&path.sequence.as_u16()) {
                duplicate_sequences.insert(path.sequence.as_u16());
            } else {
                all_sequence.insert(path.sequence.as_u16());
            }
        }
        for path in &mut self.paths {
            if duplicate_sequences.contains(&path.sequence.as_u16()) {
                path.warning = Some(PathWarning::DuplicateSequence);
            } else if path
                .keys
                .iter()
                .flatten() // can ignore None
                .any(|key| !path.kind().can_choose_key_source_kind(&key.source.kind()))
            {
                path.warning = Some(PathWarning::KeySourceKindDisallowed);
            } else if self.chain.is_blake2b()
                && path.keys.iter().all(|key| key.is_some())
                && !path.has_replay_capable_key()
            {
                // Advice, not a refusal: the path is complete and valid, but
                // every spend through it would be replayable onto Bitcoin.
                // Only once every slot is filled, so a half-built path is not
                // nagged about keys it has not chosen yet.
                path.warning = Some(PathWarning::NoReplayCapableSigner);
            } else {
                path.warning = None;
            }
        }
    }

    fn valid(&self) -> bool {
        !self.paths.iter().any(|path| {
            !path.valid()
                || (self.use_taproot
                    && path
                        .keys
                        .iter()
                        .any(|k| !k.as_ref().is_some_and(|k| k.source.is_compatible_taproot())))
        }) && self.paths.len() >= 2
    }

    fn check_setup(&mut self) {
        self.check_for_warning();
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
                DescriptorTemplate::TwoOfThreeInheritance => {
                    self.paths = vec![
                        Path::new_primary_path().with_n_keys(3).with_threshold(2),
                        Path::new_recovery_path(), // inheritance: 1 key, seq 52_596 (~1y)
                    ];
                }
                DescriptorTemplate::MultisigInheritanceRecovery => {
                    // Second recovery must have a distinct sequence from the
                    // inheritance path (apply() keys recovery paths by sequence).
                    let mut second_recovery = Path::new_recovery_path();
                    second_recovery.sequence = PathSequence::Recovery(65_000); // ~1.24y
                    self.paths = vec![
                        Path::new_primary_path().with_n_keys(3).with_threshold(2),
                        // inheritance: 2-of-3 multisig, seq 52_596 (~1y)
                        Path::new_recovery_path().with_n_keys(3).with_threshold(2),
                        second_recovery,
                    ];
                }
                DescriptorTemplate::ExpandingMultisigInheritanceRecovery => {
                    let mut second_recovery = Path::new_recovery_path();
                    second_recovery.sequence = PathSequence::Recovery(65_000); // ~1.24y
                    self.paths = vec![
                        Path::new_primary_path().with_n_keys(3).with_threshold(2),
                        // inheritance: 2-of-6 multisig — the 3 primary keys are
                        // reused (mirrored) alongside 3 inheritance keys, so any
                        // two of the six can recover. seq 52_596 (~1y).
                        Path::new_recovery_path().with_n_keys(6).with_threshold(2),
                        second_recovery,
                    ];
                }
            }
        }
        self.descriptor_template = template;
    }

    fn keys(&self) -> HashMap<Fingerprint, (Vec<(usize, usize)>, Key)> {
        let mut keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)> = self
            .keys
            .clone()
            .into_iter()
            .map(|(fg, key)| (fg, (vec![], key)))
            .collect();
        for (i, path) in self.paths.iter().enumerate() {
            for (j, key) in path.keys.iter().enumerate() {
                if let Some(k) = key {
                    let fg = k.fingerprint;
                    if !keys.contains_key(&fg) {
                        tracing::error!("DefineDesctriptor::keys() key {} missing", fg);
                    } else {
                        let entry = keys.get_mut(&fg).expect("checked");
                        entry.0.push((i, j));
                    }
                }
            }
        }
        keys
    }

    fn path_keys(&self, paths: Vec<usize>) -> Vec<Fingerprint> {
        let mut out = vec![];
        for path in paths {
            let mut fgs: Vec<_> = self.paths[path]
                .keys
                .iter()
                .filter_map(|k| k.as_ref().map(|k| k.fingerprint))
                .collect();
            out.append(&mut fgs);
        }
        out
    }

    fn apply_existing_key(&mut self, coordinates: Vec<(usize, usize)>, fingerprint: Fingerprint) {
        if let Some(existing_key) = self.keys.get(&fingerprint) {
            if !self.keys.contains_key(&fingerprint) {
                tracing::error!("Key {fingerprint} does not exists");
                return;
            }
            for coordinate in coordinates {
                if !self.paths[coordinate.0]
                    .keys
                    .contains(&Some(existing_key.clone()))
                {
                    self.paths[coordinate.0].keys[coordinate.1] = Some(existing_key.clone());
                } else {
                    tracing::error!("Key {fingerprint} already in path {}", coordinate.0);
                }
            }
            self.check_setup();
        } else {
            tracing::error!("Key with fingerprint {fingerprint} does not exists");
        }
        self.modal = None;
    }

    fn apply_selected_key(
        &mut self,
        hws: &mut HardwareWallets,
        coordinates: Vec<(usize, usize)>,
        key: key::SelectedKey,
    ) {
        match key {
            key::SelectedKey::Existing(fingerprint) => {
                self.apply_existing_key(coordinates, fingerprint);
            }
            key::SelectedKey::New(key) => {
                if let Some(acc) = key.account {
                    self.accounts.insert(key.fingerprint, acc);
                }
                if self.keys.contains_key(&key.fingerprint) {
                    tracing::error!("Key {} already exists", key.fingerprint);
                }
                self.keys.insert(key.fingerprint, *key.clone());
                hws.aliases.insert(key.fingerprint, key.name.clone());
                for coordinate in coordinates {
                    if !self.paths[coordinate.0].keys.contains(&Some(*key.clone())) {
                        self.paths[coordinate.0].keys[coordinate.1] = Some(*key.clone());
                    } else {
                        tracing::error!("Key {} already in path {}", key.fingerprint, coordinate.0);
                    }
                }
                self.check_setup();
                self.modal = None;
            }
            key::SelectedKey::None => {
                self.modal = None;
            }
        }
    }

    fn apply_key_alias(&mut self, hws: &mut HardwareWallets, fg: Fingerprint, alias: String) {
        if let Some(key) = self.keys.get_mut(&fg) {
            key.name = alias.clone();
        }
        hws.aliases.insert(fg, alias.clone());
        // FIXME: this is a bad hack, we should not store duplicates of keys in paths,
        // source of truth should be self.keys
        for p in &mut self.paths {
            #[allow(clippy::manual_flatten)]
            for k in &mut p.keys {
                if let Some(key) = k {
                    if key.fingerprint == fg {
                        key.name = alias.clone();
                    }
                }
            }
        }
        self.modal = None;
    }

    fn update_path(&mut self, i: usize, msg: message::DefinePath) -> Task<Message> {
        match msg {
            message::DefinePath::SequenceEdited(seq) => {
                self.modal = None;
                if let Some(Path {
                    sequence: PathSequence::Recovery(s),
                    ..
                }) = self.paths.get_mut(i)
                {
                    *s = seq;
                }
                self.check_for_warning();
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
                    self.check_for_warning();
                }
            }
            message::DefinePath::Key(j, msg) => {
                return self.update_path_key(i, j, msg);
            }
        }
        Task::none()
    }

    fn update_path_key(&mut self, i: usize, j: usize, msg: message::DefineKey) -> Task<Message> {
        match msg {
            message::DefineKey::Clipboard(key) => {
                return Task::perform(async move { key }, Message::Clipboard);
            }

            message::DefineKey::EditAlias => {
                let coordinates = vec![(i, j)];
                let path_kind = self.paths[i].sequence.into();
                if let Some(path) = self.paths.get(i) {
                    if let Some(Some(key)) = path.keys.get(j) {
                        let fg = key.fingerprint;
                        let alias = key.name.clone();
                        let modal =
                            EditKeyAlias::new(self.keys(), fg, alias, path_kind, coordinates);
                        self.modal = Some(Box::new(modal));
                        return Task::none();
                    }
                }
                log::error!("DefineDescriptor.update(): DefineKey::EditAlias => key is None.");
                return Task::none();
            }
            message::DefineKey::Edit => {
                let coordinates = vec![(i, j)];
                let modal = self.edit_key_modal(coordinates);
                self.modal = Some(Box::new(modal));
            }
            message::DefineKey::Delete => {
                if let Some(path) = self.paths.get_mut(i) {
                    path.keys.remove(j);
                    if path.threshold > path.keys.len() {
                        path.threshold -= 1;
                    }
                }
                // Only delete non-primary paths.
                if i > 0 // we could alternatively check `path_kind != PathKind::Primary`
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
        }
        Task::none()
    }

    fn collect_path_keys(
        &self,
        path: &Path,
        ctx: &mut Context,
        hw_is_used: &mut bool,
        key_derivation_index: &mut HashMap<Fingerprint, usize>,
    ) -> Vec<DescriptorPublicKey> {
        let mut keys: Vec<DescriptorPublicKey> = Vec::new();
        for path_key in path.keys.iter().clone() {
            let fingerprint = path_key
                .as_ref()
                .expect("Must be present at this step")
                .fingerprint;
            let key = self
                .keys
                .get(&fingerprint)
                .expect("Must be present at this step");
            if let DescriptorPublicKey::XPub(xpub) = &key.key {
                if let Some((master_fingerprint, _)) = xpub.origin {
                    ctx.keys.insert(
                        master_fingerprint,
                        KeySetting {
                            master_fingerprint,
                            name: key.name.clone(),
                            provider_key: None,
                            is_border_wallet: matches!(
                                key.source,
                                crate::installer::descriptor::KeySource::BorderWallet { .. }
                            ),
                            grid_seed_source: key.source.grid_seed_source(),
                            replay_protected: None,
                            keychain_key_id: key.source.keychain_key_id(),
                        },
                    );
                    if key.source.device_kind().is_some() {
                        *hw_is_used = true;
                    }
                }
                let derivation_index = key_derivation_index.get(&fingerprint).unwrap_or(&0);
                keys.push(DescriptorPublicKey::MultiXPub(new_multixkey_from_xpub(
                    xpub.clone(),
                    *derivation_index,
                )));
                key_derivation_index.insert(fingerprint, derivation_index + 1);
            }
        }
        keys
    }

    fn collect_recovery_paths(
        &self,
        ctx: &mut Context,
        hw_is_used: &mut bool,
        key_derivation_index: &mut HashMap<Fingerprint, usize>,
    ) -> BTreeMap<u16, PathInfo> {
        let mut recovery_paths = BTreeMap::new();

        for path in &self.paths[1..] {
            let recovery_keys = self.collect_path_keys(path, ctx, hw_is_used, key_derivation_index);

            let recovery_keys = if recovery_keys.len() == 1 {
                PathInfo::Single(recovery_keys[0].clone())
            } else {
                PathInfo::Multi(path.threshold, recovery_keys)
            };

            recovery_paths.insert(path.sequence.as_u16(), recovery_keys);
        }
        recovery_paths
    }

    fn collect_connect_vault_metadata(&self, ctx: &mut Context) {
        // Harvest keychain-sourced keys across all paths into backend-vault
        // member payloads. Dedup by fingerprint — a key reused across
        // paths still only produces one `ConnectVaultMember` row. We rely
        // on the first occurrence's path_kind (primary > recovery) as a
        // tiebreaker for the logged role hint.
        use crate::installer::context::ConnectVaultMemberPayload;
        use crate::installer::descriptor::{KeySource, KeychainKeyOwner, PathKind};
        let mut seen_fingerprints: std::collections::HashSet<Fingerprint> =
            std::collections::HashSet::new();
        for path in self.paths.iter() {
            let path_kind = path.sequence.path_kind();
            for maybe_key in path.keys.iter() {
                let Some(editor_key_ref) = maybe_key.as_ref() else {
                    continue;
                };
                let fingerprint = editor_key_ref.fingerprint;
                if !seen_fingerprints.insert(fingerprint) {
                    continue;
                }
                let Some(key) = self.keys.get(&fingerprint) else {
                    continue;
                };
                let KeySource::KeychainKey { owner, key_id, .. } = &key.source else {
                    // HW, xpub, master-signer, token, and border-wallet
                    // keys have no backend row — they stay local-only per
                    // the 2026-04-18 plan direction. Logged at `debug!`:
                    // this is an informational "skipped as expected"
                    // message, not actionable, and we'd rather keep
                    // per-wallet identifiers out of info-level telemetry.
                    tracing::debug!(
                        "Skipping non-keychain key {} for backend vault (source kind = {:?}); \
                         backend ConnectVault will be local-key-free for this signer",
                        fingerprint,
                        key.source.kind()
                    );
                    continue;
                };
                let contact_id = match owner {
                    KeychainKeyOwner::SelfUser { .. } => None,
                    KeychainKeyOwner::Contact { contact_id, .. } => Some(*contact_id),
                };
                ctx.connect_vault_members.push(ConnectVaultMemberPayload {
                    fingerprint,
                    key_id: *key_id,
                    contact_id,
                    path_kind,
                });
            }
        }

        // Approximate timelock from the longest Recovery path's sequence
        // (blocks). Skip SafetyNet (u16::MAX) — it's not a real timelock.
        let longest_recovery_blocks = self
            .paths
            .iter()
            .filter(|p| p.sequence.path_kind() == PathKind::Recovery)
            .map(|p| p.sequence.as_u16() as u32)
            .max();
        ctx.connect_vault_timelock_days = longest_recovery_blocks.map(|blocks| {
            // Bitcoin averages ~144 blocks/day. Round up so the server
            // timelock never expires before the on-chain timelock.
            let days = blocks.div_ceil(144);
            days.max(1) as i32
        });
    }

    fn reopen_key_modal(&mut self, coordinates: Vec<(usize, usize)>) {
        // Every coordinate in the Vec belongs to the same path,
        // by construction of the picker call sites.
        if let Some((path_idx, _)) = coordinates.first() {
            if self.paths.get(*path_idx).is_some() {
                let modal = self.edit_key_modal(coordinates);
                self.modal = Some(Box::new(modal));
            }
        }
    }

    fn edit_key_modal(&self, coordinates: Vec<(usize, usize)>) -> SelectKeySource {
        let keys = self.path_keys(coordinates.iter().map(|c| c.0).collect());
        let actual_path = PathData { coordinates, keys };
        let keys = self.keys();
        SelectKeySource::new_for_chain(
            self.chain,
            self.use_taproot,
            actual_path,
            keys,
            self.accounts.clone(),
            self.signer.clone(),
            self.cube_id.clone(),
            self.coincube_client.clone(),
            self.cube_encryption_key.clone(),
        )
    }
}

impl Step for DefineDescriptor {
    fn vault_stage(&self) -> Option<crate::installer::VaultStage> {
        Some(crate::installer::VaultStage::Keys)
    }
    fn load_context(&mut self, ctx: &Context) {
        self.chain = ctx.bitcoin_config.chain;
        self.network = ctx.bitcoin_config.network;
        if self.chain.is_blake2b() {
            self.use_taproot = false;
        }
        #[cfg(feature = "mcp-proposals")]
        if let Some(proposal) = &ctx.mcp_proposal {
            if self.paths.is_empty() {
                let topology = proposal.topology();
                let mut recovery = Path::new_recovery_path()
                    .with_n_keys(usize::from(topology.recovery.quorum.key_count))
                    .with_threshold(usize::from(topology.recovery.quorum.threshold));
                recovery.sequence = PathSequence::Recovery(topology.recovery.relative_delay_blocks);
                self.paths = vec![
                    Path::new_primary_path().with_n_keys(5).with_threshold(3),
                    recovery,
                ];
                self.descriptor_template = DescriptorTemplate::Custom;
                match proposal.creation_keys() {
                    Ok(Some(keys)) => {
                        self.paths[0].keys = keys.primary.iter().cloned().map(Some).collect();
                        self.paths[1].keys = keys.recovery.iter().cloned().map(Some).collect();
                        for key in keys.primary.into_iter().chain(keys.recovery) {
                            self.keys.insert(key.fingerprint, key);
                        }
                        self.check_setup();
                    }
                    Ok(None) => {},
                    Err(_) => self.error = Some("Wallet preparation expired or its public keys are invalid. Prepare a fresh proposal.".into()),
                }
            }
            self.use_taproot = false;
        } else {
            self.load_template(ctx.descriptor_template);
        }
        #[cfg(not(feature = "mcp-proposals"))]
        self.load_template(ctx.descriptor_template);
        self.cube_id = ctx.cube_id.clone();
        self.coincube_client = ctx.coincube_client.clone();
        self.cube_encryption_key = ctx.cube_encryption_key.clone();
    }
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        self.error = None;
        match message {
            Message::Close => {
                self.modal = None;
            }
            Message::CreateTaprootDescriptor(use_taproot) => {
                if use_taproot && self.chain.is_blake2b() {
                    self.error =
                        Some("Taproot creation is unavailable for Bitcoin Blake2b".to_string());
                    return Task::none();
                }
                self.use_taproot = use_taproot;
                self.check_setup();
            }
            Message::DefineDescriptor(message::DefineDescriptor::ChangeTemplate(template)) => {
                self.descriptor_template = template;
            }
            Message::DefineDescriptor(message::DefineDescriptor::AddRecoveryPath) => {
                self.paths.push(Path::new_recovery_path());
            }
            Message::DefineDescriptor(message::DefineDescriptor::OpenBorderWalletWizard(
                coordinates,
            )) => {
                let modal = border_wallet_wizard::BorderWalletWizard::new(
                    self.network,
                    coordinates,
                    self.signer.clone(),
                    false, // Default: use master-derived grid phrase
                );
                self.modal = Some(Box::new(modal));
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdited(coordinates, key)) => {
                self.apply_selected_key(hws, coordinates, key);
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeysEdit(_, coordinates)) => {
                let modal = self.edit_key_modal(coordinates);
                self.modal = Some(Box::new(modal));
            }
            Message::DefineDescriptor(message::DefineDescriptor::ReopenKeyModal(coordinates)) => {
                self.reopen_key_modal(coordinates);
            }
            Message::DefineDescriptor(message::DefineDescriptor::AliasEdited(fg, alias)) => {
                self.apply_key_alias(hws, fg, alias);
            }
            Message::DefineDescriptor(message::DefineDescriptor::Reset) => {
                hws.aliases.clear();
                self.keys.clear();
                self.paths.clear();
                self.load_template(self.descriptor_template);
                self.modal = None;
            }
            Message::DefineDescriptor(message::DefineDescriptor::Path(i, msg)) => {
                return self.update_path(i, msg);
            }
            Message::EditKeyAlias(msg) => match msg.clone() {
                key::EditKeyAliasMessage::Alias(_)
                | key::EditKeyAliasMessage::Save
                | key::EditKeyAliasMessage::Replace => {
                    if let Some(modal) = &mut self.modal {
                        return modal.update(hws, Message::EditKeyAlias(msg));
                    }
                }
                key::EditKeyAliasMessage::DoReplace { coordinates, .. } => {
                    let modal = self.edit_key_modal(coordinates);
                    self.modal = Some(Box::new(modal));
                }
                key::EditKeyAliasMessage::Close => self.modal = None,
            },
            _ => {
                if let Some(modal) = &mut self.modal {
                    return modal.update(hws, message);
                }
            }
        }
        Task::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        if let Some(modal) = &self.modal {
            modal.subscription(hws)
        } else {
            Subscription::none()
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if ctx.bitcoin_config.chain != self.chain || (self.chain.is_blake2b() && self.use_taproot) {
            self.error = Some("Descriptor chain or type is unsupported".to_string());
            return false;
        }
        if self
            .paths
            .iter()
            .flat_map(|path| path.keys.iter())
            .flatten()
            .any(|key| !key.source.available_for_creation(self.chain))
        {
            self.error =
                Some("This key source is not available for Bitcoin Blake2b yet".to_string());
            return false;
        }
        if self.paths.len() < 2 {
            return false;
        }

        // Through the context, so the chain identity moves with the encoding.
        if let Err(e) = ctx.set_bitcoin_network(self.network) {
            self.error = Some(e);
            return false;
        }
        ctx.keys = HashMap::new();
        // Every key below comes with its source, so a key recorded without a
        // Keychain id is known not to be one.
        ctx.keychain_keys_recorded = true;
        // Reset backend-vault payloads — `apply()` runs every time the
        // user re-confirms the descriptor, including after going back
        // and editing, so we can't append incrementally.
        ctx.connect_vault_members = Vec::new();
        ctx.connect_vault_timelock_days = None;
        let mut hw_is_used = false;
        let mut key_derivation_index = HashMap::<Fingerprint, usize>::new();
        let spending_keys = self.collect_path_keys(
            &self.paths[0],
            ctx,
            &mut hw_is_used,
            &mut key_derivation_index,
        );

        let recovery_paths =
            self.collect_recovery_paths(ctx, &mut hw_is_used, &mut key_derivation_index);

        if spending_keys.is_empty() {
            return false;
        }

        self.collect_connect_vault_metadata(ctx);

        let spending_keys = if spending_keys.len() == 1 {
            PathInfo::Single(spending_keys[0].clone())
        } else {
            PathInfo::Multi(self.paths[0].threshold, spending_keys)
        };

        let policy = match if self.use_taproot {
            CoincubePolicy::new(spending_keys, recovery_paths)
        } else {
            CoincubePolicy::new_legacy(spending_keys, recovery_paths)
        } {
            Ok(policy) => policy,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };

        ctx.descriptor = Some(CoincubeDescriptor::new(policy));
        ctx.hw_is_used = hw_is_used;
        true
    }

    fn view<'a>(
        &'a self,
        hws: &'a HardwareWallets,
        progress: (usize, usize),
    ) -> Element<'a, Message> {
        let content = match self.descriptor_template {
            DescriptorTemplate::SimpleInheritance => {
                view::editor::template::inheritance::inheritance_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &self.paths[1],
                    self.valid(),
                )
            }
            DescriptorTemplate::MultisigSecurity => {
                view::editor::template::multisig_security_wallet::multisig_security_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &self.paths[1],
                    self.valid(),
                )
            }
            // Bespoke mirroring editor: primary keys are reused in the 2-of-6
            // inheritance path (paths[1]), plus a single backup (paths[2]).
            DescriptorTemplate::ExpandingMultisigInheritanceRecovery => {
                view::editor::template::multisig_security_wallet::expanding_multisig_inheritance_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &self.paths[1],
                    &self.paths[2],
                    self.valid(),
                )
            }
            DescriptorTemplate::TwoOfThreeInheritance => {
                view::editor::template::inheritance::two_of_three_inheritance_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &self.paths[1],
                    self.valid(),
                )
            }
            DescriptorTemplate::MultisigInheritanceRecovery => {
                view::editor::template::multisig_security_wallet::multisig_inheritance_recovery_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &self.paths[1],
                    &self.paths[2],
                    self.valid(),
                )
            }
            DescriptorTemplate::Custom => {
                view::editor::template::custom::custom_template(
                    progress,
                    self.use_taproot,
                    !self.chain.is_blake2b(),
                    &self.paths[0],
                    &mut self.paths[1..]
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.kind() == PathKind::Recovery),
                    self.paths[1..]
                        .iter()
                        .enumerate()
                        .find(|(_, p)| p.kind() == PathKind::SafetyNet),
                    self.paths[1..].len(),
                    self.valid(),
                )
            }
        };
        if let Some(modal) = &self.modal {
            // The step sits in the Create Vault frame's scrolled column, whose
            // bounds are smaller than the Border Wallet sheet: lay the modal
            // out against the window so its footer is never clipped.
            Modal::new(content, modal.view(hws))
                .cover_window()
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
    pub fn new(path_index: usize, path_sequence: PathSequence) -> Self {
        Self {
            path_index,
            sequence: form::Value {
                value: path_sequence.as_u16().to_string(),
                warning: None,
                valid: true,
            },
        }
    }
}

impl DescriptorEditModal for EditSequenceModal {
    fn processing(&self) -> bool {
        false
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
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
                message::ThresholdSequenceModal::Confirm if self.sequence.valid => {
                    if let Ok(sequence) = u16::from_str(&self.sequence.value) {
                        let path_index = self.path_index;
                        return Task::perform(
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
                _ => {}
            }
        }
        Task::none()
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

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
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
                    return Task::perform(
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
        Task::none()
    }

    fn view(&self, _hws: &HardwareWallets) -> Element<Message> {
        view::editor::edit_threshold_modal(self.threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::StreamExt;
    use iced_runtime::{task::into_stream, Action};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use crate::installer::step::descriptor::editor::key::{SelectKeySource, SelectedKey};
    use crate::{dir::CoincubeDirectory, installer::descriptor::KeySource};

    /// The Border Wallet pattern sheet, opened from the real editor and drawn
    /// inside the Create Vault frame, keeps all 16 grid columns and its
    /// Continue inside the minimum window once the pattern is complete. The
    /// editor's modal must cover the window: against the step's own bounds
    /// the sheet is squeezed and the last columns collapse to nothing.
    #[test]
    fn border_wallet_pattern_sheet_fits_the_create_vault_frame() {
        use border_wallet_wizard::BorderWalletWizardMessage as Bw;
        use iced::advanced::widget::{operation, operation::focusable, Id, Operation};
        use iced::{Rectangle, Size};
        use iced_runtime::user_interface::{Cache, UserInterface};

        #[derive(Default)]
        struct Probe {
            next: Option<Rectangle>,
            column_16: Vec<Rectangle>,
        }
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
                if value == "16" {
                    self.column_16.push(bounds);
                }
            }
            fn focusable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                _: &mut dyn focusable::Focusable,
            ) {
                if id == Some(&Id::new("border-wallet.pattern.next")) {
                    self.next = Some(bounds);
                }
            }
        }

        let network = coincube_core::miniscript::bitcoin::Network::Regtest;
        let dir = CoincubeDirectory::new(PathBuf::new());
        let signer = Arc::new(Mutex::new(Signer::generate(network).unwrap()));
        let mut step = DefineDescriptor::new(network, signer);
        step.load_template(DescriptorTemplate::SimpleInheritance);
        let mut hws = HardwareWallets::new(dir, network);
        let mut send = |step: &mut DefineDescriptor, message: Message| {
            let _ = step.update(&mut hws, message);
        };
        send(
            &mut step,
            Message::DefineDescriptor(message::DefineDescriptor::OpenBorderWalletWizard(vec![(
                0, 0,
            )])),
        );
        send(&mut step, Message::BorderWalletWizard(Bw::Next));
        // A public BIP39 test vector, never a user's recovery phrase.
        let words = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        for (index, word) in words.split(' ').enumerate() {
            send(
                &mut step,
                Message::BorderWalletWizard(Bw::PhraseWordEdited(index, word.to_string())),
            );
        }
        send(&mut step, Message::BorderWalletWizard(Bw::Next));
        for col in 0..coincube_core::border_wallet::PATTERN_LENGTH as u8 {
            send(
                &mut step,
                Message::BorderWalletWizard(Bw::ToggleCellSelection(0, col)),
            );
        }

        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                coincube_ui::font::REGULAR,
                iced::Pixels(14.0),
                Some("tiny-skia"),
            ),
        )
        .unwrap();
        let window = Size::new(1024.0, 768.0);
        let _framed =
            crate::installer::view::FramedGuard::enter(crate::installer::CREATE_VAULT_TITLE);
        // As the Create Vault frame draws a step: inside its scrolled column.
        let element: Element<'_, Message> = iced::widget::scrollable(
            coincube_ui::widget::Container::new(step.view(&hws, (0, 0))).padding([40, 24]),
        )
        .into();
        let mut ui = UserInterface::build(element, window, Cache::new(), &mut renderer);
        let mut probe = Probe::default();
        ui.operate(&renderer, &mut operation::black_box(&mut probe));
        let next = probe.next.expect("the pattern step's Continue is enabled");
        assert!(
            next.x >= 0.0
                && next.y >= 0.0
                && next.x + next.width <= window.width
                && next.y + next.height <= window.height,
            "Continue at {:?} is outside the {:?} window",
            next,
            window
        );
        // "16" is both the last column's header (46 px) and row 16's label.
        assert!(
            probe.column_16.iter().any(|bounds| bounds.width == 46.0),
            "column 16 is squeezed: {:?}",
            probe.column_16
        );
    }

    #[test]
    fn fork_descriptor_refuses_taproot_messages_and_stale_state() {
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let dir = CoincubeDirectory::new(PathBuf::new());
            let signer = Arc::new(Mutex::new(
                Signer::generate(chain.bitcoin_network()).unwrap(),
            ));
            let mut ctx = Context::new_for_chain(chain, dir.clone(), None, None);
            let mut step = DefineDescriptor::new(chain.bitcoin_network(), signer);
            step.use_taproot = true;
            step.load_context(&ctx);
            assert!(!step.use_taproot);
            let mut hws = HardwareWallets::new(dir, chain.bitcoin_network());
            let _ = step.update(&mut hws, Message::CreateTaprootDescriptor(true));
            assert!(!step.use_taproot);
            assert!(step.error.is_some());
            let task = step.update(
                &mut hws,
                Message::DefineDescriptor(message::DefineDescriptor::OpenBorderWalletWizard(vec![
                    (0, 0),
                ])),
            );
            // HW-1 Amendment A: the Border Wallet wizard opens on Bitcoin
            // Blake2b as it does on Bitcoin.
            assert!(into_stream(task).is_none());
            assert!(step.error.is_none());
            assert!(step.modal.is_some());
            step.modal = None;
            step.use_taproot = true;
            assert!(!step.apply(&mut ctx));
            assert!(ctx.descriptor.is_none());
            step.use_taproot = false;
            let xpub = step.signer.lock().unwrap().get_extended_pubkey(
                &coincube_core::miniscript::bitcoin::bip32::DerivationPath::master(),
            );
            step.paths[0].keys = vec![Some(Key {
                source: KeySource::KeychainKey {
                    owner: crate::installer::descriptor::KeychainKeyOwner::SelfUser {
                        primary_owner_id: 1,
                    },
                    key_id: 1,
                    name: "phone".to_string(),
                },
                name: "phone".to_string(),
                fingerprint: step.signer.lock().unwrap().fingerprint(),
                key: DescriptorPublicKey::from_str(&xpub.to_string()).unwrap(),
                account: None,
            })];
            assert!(!step.apply(&mut ctx));
            assert!(step.error.as_ref().unwrap().contains("key source"));
            assert!(ctx.descriptor.is_none());
            assert!(KeySource::Manual.available_for_creation(chain));
            assert!(KeySource::MasterSigner.available_for_creation(chain));
        }
    }

    /// Every key source the creation gate knows, one of each.
    fn every_key_source() -> Vec<KeySource> {
        use crate::app::settings::GridSeedSource;
        vec![
            KeySource::MasterSigner,
            KeySource::Manual,
            KeySource::Device(async_hwi::DeviceKind::Ledger, None),
            KeySource::Device(async_hwi::DeviceKind::Specter, None),
            KeySource::KeychainKey {
                owner: crate::installer::descriptor::KeychainKeyOwner::SelfUser {
                    primary_owner_id: 1,
                },
                key_id: 1,
                name: "phone".to_string(),
            },
            KeySource::BorderWallet {
                grid_seed_source: GridSeedSource::Independent,
            },
        ]
    }

    /// HW-1 and its Amendment A (Robert, 2026-10-06): on both Bitcoin
    /// Blake2b chains the Cube key, xpubs, hardware devices and Border
    /// Wallet keys may build a Vault; Keychain keys may not. Every source
    /// stays available on every Bitcoin-family chain.
    #[test]
    fn creation_gate_on_bitcoin_blake2b_refuses_only_keychain_sources() {
        use crate::chain::ChainId;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            for source in every_key_source() {
                let expected = matches!(
                    source,
                    KeySource::MasterSigner
                        | KeySource::Manual
                        | KeySource::Device(..)
                        | KeySource::BorderWallet { .. }
                );
                assert_eq!(
                    source.available_for_creation(chain),
                    expected,
                    "{:?} {:?}",
                    chain,
                    source
                );
            }
        }
        for chain in [
            ChainId::Bitcoin,
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ] {
            for source in every_key_source() {
                assert!(
                    source.available_for_creation(chain),
                    "{:?} {:?}",
                    chain,
                    source
                );
            }
        }
    }

    /// HW-1: a Bitcoin Blake2b descriptor with a hardware device key on its
    /// primary path applies, and records that a device is used so the
    /// registration step runs. The same descriptor with a Keychain key in
    /// the device's place is still refused.
    #[test]
    fn fork_descriptor_applies_with_a_device_key_and_refuses_a_keychain_key() {
        use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let network = chain.bitcoin_network();
            let coin = if network == Network::Bitcoin { 0 } else { 1 };
            let key_from = |signer: &Signer, source: KeySource, name: &str| {
                let path = DerivationPath::from_str(&format!("m/48'/{coin}'/0'/2'")).unwrap();
                Key {
                    source,
                    name: name.to_string(),
                    fingerprint: signer.fingerprint(),
                    key: DescriptorPublicKey::from_str(&format!(
                        "[{}/48'/{coin}'/0'/2']{}",
                        signer.fingerprint(),
                        signer.get_extended_pubkey(&path)
                    ))
                    .unwrap(),
                    account: None,
                }
            };
            let device_signer = Signer::generate(network).unwrap();
            let backup_signer = Signer::generate(network).unwrap();
            let device = key_from(
                &device_signer,
                KeySource::Device(async_hwi::DeviceKind::Ledger, None),
                "Ledger",
            );
            let backup = key_from(&backup_signer, KeySource::Manual, "Backup");

            let dir = CoincubeDirectory::new(PathBuf::new());
            let mut ctx = Context::new_for_chain(chain, dir, None, None);
            let mut step = DefineDescriptor::new(
                network,
                Arc::new(Mutex::new(Signer::generate(network).unwrap())),
            );
            step.load_context(&ctx);
            step.keys.insert(device.fingerprint, device.clone());
            step.keys.insert(backup.fingerprint, backup.clone());
            step.paths[0].keys = vec![Some(device.clone())];
            step.paths[0].threshold = 1;
            step.paths[1].keys = vec![Some(backup.clone())];
            step.paths[1].threshold = 1;
            assert!(step.apply(&mut ctx), "{chain:?}: {:?}", step.error);
            assert!(step.error.is_none());
            assert!(ctx.descriptor.is_some());
            assert!(
                ctx.hw_is_used,
                "{:?}: the registration step must run",
                chain
            );
            assert_eq!(ctx.bitcoin_config.chain, chain);

            // A Keychain key in the device's place is still refused.
            let mut ctx =
                Context::new_for_chain(chain, CoincubeDirectory::new(PathBuf::new()), None, None);
            let mut phone = device.clone();
            phone.source = KeySource::KeychainKey {
                owner: crate::installer::descriptor::KeychainKeyOwner::SelfUser {
                    primary_owner_id: 1,
                },
                key_id: 1,
                name: "phone".to_string(),
            };
            step.keys.insert(phone.fingerprint, phone.clone());
            step.paths[0].keys = vec![Some(phone)];
            assert!(!step.apply(&mut ctx), "{:?}", chain);
            assert_eq!(
                step.error.as_deref(),
                Some("This key source is not available for Bitcoin Blake2b yet")
            );
            assert!(ctx.descriptor.is_none());

            // Amendment A: a Border Wallet key in the device's place applies.
            let mut ctx =
                Context::new_for_chain(chain, CoincubeDirectory::new(PathBuf::new()), None, None);
            let mut border = device.clone();
            border.source = KeySource::BorderWallet {
                grid_seed_source: crate::app::settings::GridSeedSource::Independent,
            };
            step.keys.insert(border.fingerprint, border.clone());
            step.paths[0].keys = vec![Some(border)];
            assert!(step.apply(&mut ctx), "{chain:?}: {:?}", step.error);
            assert!(ctx.descriptor.is_some());
            assert!(ctx.keys[&device_signer.fingerprint()].is_border_wallet);
            assert!(!ctx.hw_is_used);
        }
    }

    /// A Vault built in the editor records which of its keys came from the
    /// Keychain, on every path, and marks that record complete.
    #[test]
    fn apply_records_keychain_provenance_per_key() {
        use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
        let network = Network::Signet;
        let key_from = |signer: &Signer, source: KeySource, name: &str| {
            let path = DerivationPath::from_str("m/48'/1'/0'/2'").unwrap();
            let xpub = signer.get_extended_pubkey(&path);
            Key {
                source,
                name: name.to_string(),
                fingerprint: signer.fingerprint(),
                key: DescriptorPublicKey::from_str(&format!(
                    "[{}/48'/1'/0'/2']{}",
                    signer.fingerprint(),
                    xpub
                ))
                .unwrap(),
                account: None,
            }
        };
        let phone = Signer::generate(network).unwrap();
        let backup = Signer::generate(network).unwrap();
        let phone_key = key_from(
            &phone,
            KeySource::KeychainKey {
                owner: crate::installer::descriptor::KeychainKeyOwner::SelfUser {
                    primary_owner_id: 1,
                },
                key_id: 42,
                name: "My iPhone".to_string(),
            },
            "My iPhone",
        );
        let backup_key = key_from(&backup, KeySource::Manual, "Backup");

        let dir = CoincubeDirectory::new(PathBuf::new());
        let mut ctx = Context::new(network, dir, None, None);
        let mut step = DefineDescriptor::new(
            network,
            Arc::new(Mutex::new(Signer::generate(network).unwrap())),
        );
        step.load_context(&ctx);
        step.keys.insert(phone_key.fingerprint, phone_key.clone());
        step.keys.insert(backup_key.fingerprint, backup_key.clone());
        step.paths[0].keys = vec![Some(phone_key.clone())];
        step.paths[0].threshold = 1;
        step.paths[1].keys = vec![Some(backup_key.clone())];
        step.paths[1].threshold = 1;
        assert!(step.apply(&mut ctx), "{:?}", step.error);

        assert!(ctx.keychain_keys_recorded);
        assert_eq!(ctx.keys[&phone.fingerprint()].keychain_key_id, Some(42));
        assert_eq!(ctx.keys[&backup.fingerprint()].keychain_key_id, None);
    }

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
            let mut hws = HardwareWallets::new(
                CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
                Network::Bitcoin,
            );
            let cmd = self.step.lock().unwrap().update(&mut hws, message);
            if let Some(mut stream) = into_stream(cmd) {
                while let Some(action) = stream.next().await {
                    if let Action::Output(msg) = action {
                        let _cmd = self.step.lock().unwrap().update(&mut hws, msg);
                    }
                }
            }
        }
        pub async fn load(&self, ctx: &Context) {
            self.step.lock().unwrap().load_context(ctx);
        }
    }

    #[tokio::test]
    async fn test_define_descriptor_use_masterkey() {
        let mut ctx = Context::new(
            Network::Signet,
            CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
            None,
            None,
        );
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(
            Network::Signet,
            Arc::new(Mutex::new(Signer::generate(Network::Signet).unwrap())),
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
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectGenerateMasterKey,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "master_signer_key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
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
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectEnterXpub,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::Xpub("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK".to_string())
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "External recovery key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
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
            CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
            None,
            None,
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
            source: KeySource::Device(async_hwi::DeviceKind::Specter, None),
            account: None,
        };

        // Use Specter device for primary key
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeysEdited(
                    vec![(0, 0)],
                    SelectedKey::New(Box::new(specter_key.clone())),
                ),
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
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectEnterXpub,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::Xpub("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK".to_string())
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "External recovery key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
            .await;
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });

        // Now edit primary key to use master signer instead of Specter device
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                0,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
            .await;
        sandbox.check(|step| assert!(step.modal.is_some()));
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectGenerateMasterKey,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "master signer key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
            .await;
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!((step).apply(&mut ctx));
            assert!(!ctx.hw_is_used);
        });

        // Now edit the recovery key to use Specter device
        sandbox
            .update(Message::DefineDescriptor(
                message::DefineDescriptor::KeysEdited(
                    vec![(1, 0)],
                    SelectedKey::New(Box::new(specter_key.clone())),
                ),
            ))
            .await;
        sandbox.check(|step| {
            assert!((step).apply(&mut ctx));
            assert!(ctx.hw_is_used);
        });
    }

    /// The descriptor-kind selector round-trips: P2WSH is the default, picking
    /// Taproot compiles creation output to `tr(...)`, and picking P2WSH back
    /// returns it to `wsh(...)`. This replaces the no-op assertion that stood
    /// while creation was P2WSH-only for BIP-110 rule-7 reasons.
    #[tokio::test]
    async fn test_define_descriptor_kind_round_trips() {
        let mut ctx = Context::new(
            Network::Signet,
            CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
            None,
            None,
        );
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(
            Network::Signet,
            Arc::new(Mutex::new(Signer::generate(Network::Signet).unwrap())),
        ));
        sandbox.load(&ctx).await;

        // P2WSH is the default; the selector moves it and moves it back.
        sandbox.check(|step| assert!(!step.use_taproot));
        sandbox.update(Message::CreateTaprootDescriptor(true)).await;
        sandbox.check(|step| assert!(step.use_taproot));
        sandbox
            .update(Message::CreateTaprootDescriptor(false))
            .await;
        sandbox.check(|step| assert!(!step.use_taproot));

        // Primary key: generated master key.
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                0,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectGenerateMasterKey,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "master_signer_key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
            .await;

        // Recovery path: timelock sequence + external xpub.
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                message::DefinePath::SequenceEdited(1000),
            )))
            .await;
        sandbox
            .update(Message::DefineDescriptor(message::DefineDescriptor::Path(
                1,
                message::DefinePath::Key(0, message::DefineKey::Edit),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::SelectEnterXpub,
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(
                key::SelectKeySourceMessage::Xpub("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK".to_string())
            ))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Alias(
                "External recovery key".to_string(),
            )))
            .await;
        sandbox
            .update(SelectKeySource::route(key::SelectKeySourceMessage::Next))
            .await;

        // Left on the default, creation compiles to P2WSH.
        sandbox.check(|step| {
            assert!(step.modal.is_none());
            assert!(!step.use_taproot);
            assert!((step).apply(&mut ctx));
            let desc = ctx.descriptor.as_ref().unwrap().to_string();
            assert!(
                desc.starts_with("wsh("),
                "the default must produce a P2WSH descriptor, got: {}",
                desc
            );
        });

        // Switched to Taproot, the same paths compile to `tr(...)` — the
        // capability the P2WSH-only period removed from the creation flow.
        sandbox.update(Message::CreateTaprootDescriptor(true)).await;
        sandbox.check(|step| {
            assert!(step.use_taproot);
            assert!((step).apply(&mut ctx));
            let desc = ctx.descriptor.as_ref().unwrap().to_string();
            assert!(
                desc.starts_with("tr("),
                "picking Taproot must produce a Taproot descriptor, got: {}",
                desc
            );
        });
    }

    /// Bitcoin Blake2b Vault creation warns — without refusing — when a
    /// complete path has no signer that can make replay-protected
    /// signatures (`#276` I3, desktop plan PR 6). Devices stay selectable and
    /// the path stays valid; on a Bitcoin-family chain nothing is added.
    #[tokio::test]
    async fn blake2b_creation_notices_paths_without_a_replay_capable_signer() {
        use crate::installer::descriptor::{Path, PathWarning};

        let device_key = DescriptorPublicKey::from_str("[4df3f0e3/84'/0'/0']tpubDDRs9DnRUiJc4hq92PSJKhfzQBgHJUrDo7T2i48smsDfLsQcm3Vh7JhuGqJv8zozVkNFin8YPgpmn2NWNmpRaE3GW2pSxbmAzYf2juy7LeW").unwrap();
        let device = Key {
            name: "Specter".to_string(),
            fingerprint: device_key.master_fingerprint(),
            key: device_key,
            source: KeySource::Device(async_hwi::DeviceKind::Specter, None),
            account: None,
        };
        let manual_key = DescriptorPublicKey::from_str("[f5acc2fd/48'/1'/0'/2']tpubDFAqEGNyad35aBCKUAXbQGDjdVhNueno5ZZVEn3sQbW5ci457gLR7HyTmHBg93oourBssgUxuWz1jX5uhc1qaqFo9VsybY1J5FuedLfm4dK").unwrap();
        let manual = Key {
            name: "Pasted".to_string(),
            fingerprint: manual_key.master_fingerprint(),
            key: manual_key,
            source: KeySource::Manual,
            account: None,
        };
        let hot = Key {
            name: "Cube key".to_string(),
            fingerprint: manual.fingerprint,
            key: manual.key.clone(),
            source: KeySource::MasterSigner,
            account: None,
        };

        let mut ctx = Context::new(
            Network::Bitcoin,
            CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
            None,
            None,
        );
        ctx.bitcoin_config.chain = crate::chain::ChainId::BitcoinBlake2b;
        let sandbox: Sandbox<DefineDescriptor> = Sandbox::new(DefineDescriptor::new(
            Network::Bitcoin,
            Arc::new(Mutex::new(Signer::generate(Network::Bitcoin).unwrap())),
        ));
        sandbox.load(&ctx).await;

        sandbox.check(|step| {
            assert_eq!(step.chain, crate::chain::ChainId::BitcoinBlake2b);
            let mut primary = Path::new_primary_path();
            primary.keys = vec![Some(device.clone())];
            let mut recovery = Path::new_recovery_path();
            recovery.keys = vec![Some(manual.clone())];
            step.paths = vec![primary, recovery];
            step.check_for_warning();
            for path in &step.paths {
                assert_eq!(path.warning, Some(PathWarning::NoReplayCapableSigner));
                assert!(path.valid(), "the notice never blocks the path");
            }
            assert!(step.valid());
            assert!(!PathWarning::NoReplayCapableSigner.blocks());
            assert!(PathWarning::NoReplayCapableSigner
                .message()
                .contains("replayed onto Bitcoin unless the coins were split first"));

            // A half-built path is not nagged about keys it has not chosen.
            step.paths[0].keys.push(None);
            step.check_for_warning();
            assert_eq!(step.paths[0].warning, None);
            step.paths[0].keys.pop();

            // The Cube key on the primary path clears its notice; the
            // recovery path keeps its own.
            step.paths[0].keys = vec![Some(hot.clone()), Some(device.clone())];
            step.check_for_warning();
            assert_eq!(step.paths[0].warning, None);
            assert_eq!(
                step.paths[1].warning,
                Some(PathWarning::NoReplayCapableSigner)
            );

            // A Border Wallet key can sign protected on its own, so a path
            // holding only one gets no notice (HW-1 Amendment A).
            let border = Key {
                source: KeySource::BorderWallet {
                    grid_seed_source: crate::app::settings::GridSeedSource::Independent,
                },
                ..device.clone()
            };
            step.paths[0].keys = vec![Some(border)];
            step.check_for_warning();
            assert_eq!(step.paths[0].warning, None);
            step.paths[0].keys = vec![Some(hot.clone()), Some(device.clone())];
            step.check_for_warning();

            // Blocking warnings still win over the notice.
            step.paths[1].sequence = crate::installer::descriptor::PathSequence::Recovery(10);
            step.paths.push({
                let mut duplicate = Path::new_recovery_path();
                duplicate.sequence = crate::installer::descriptor::PathSequence::Recovery(10);
                duplicate.keys = vec![Some(manual.clone())];
                duplicate
            });
            step.check_for_warning();
            assert_eq!(step.paths[1].warning, Some(PathWarning::DuplicateSequence));
            assert!(!step.paths[1].valid());
            step.paths.pop();

            // Bitcoin family: no notice, whatever the keys.
            step.chain = crate::chain::ChainId::Bitcoin;
            step.paths[0].keys = vec![Some(device.clone())];
            step.check_for_warning();
            assert!(step.paths.iter().all(|p| p.warning.is_none()));
        });
    }

    #[test]
    fn replay_capable_key_sources_are_the_in_process_ones() {
        use crate::app::settings::GridSeedSource;
        assert!(KeySource::MasterSigner.replay_capable());
        assert!(KeySource::BorderWallet {
            grid_seed_source: GridSeedSource::Independent
        }
        .replay_capable());
        assert!(!KeySource::Device(async_hwi::DeviceKind::Specter, None).replay_capable());
        assert!(!KeySource::Manual.replay_capable());
        assert!(!KeySource::KeychainKey {
            owner: crate::installer::descriptor::KeychainKeyOwner::SelfUser {
                primary_owner_id: 1,
            },
            key_id: 1,
            name: "phone".to_string(),
        }
        .replay_capable());
    }
}
