//! Native review and chain-bound creation admission. No MCP setter can approve.
use super::{Context, Error, Message, Step};
use coincube_core::{
    descriptors::PathInfo,
    miniscript::bitcoin::hashes::{sha256, Hash},
};
use coincube_mcp::{
    ipc::Grant,
    proposal::{Proposal, Quorum, Store, Topology},
};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::text::{h3, text},
    widget::Element,
};
use iced::{
    widget::{button, column, scrollable},
    Task,
};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{Arc, Mutex, OnceLock},
};

/// Private native session; intentionally not Debug/Serialize and never a DTO.
pub(crate) struct Session {
    cube_id: String,
    generation: u64,
    chain: crate::chain::ChainId,
    grant: Arc<Grant>,
    store: Arc<Store>,
    proposal: Proposal,
    approval: Mutex<Option<sha256::Hash>>,
}
impl Session {
    pub(crate) fn new(
        cube_id: String,
        generation: u64,
        chain: crate::chain::ChainId,
        grant: Arc<Grant>,
        store: Arc<Store>,
        proposal: Proposal,
    ) -> Self {
        Self {
            cube_id,
            generation,
            chain,
            grant,
            store,
            proposal,
            approval: Mutex::new(None),
        }
    }
    pub(crate) fn chain(&self) -> crate::chain::ChainId {
        self.chain
    }
    pub(crate) fn topology(&self) -> Topology {
        self.proposal.topology
    }
    /// Immutable public keys from the separately paired preparation scope.
    pub(crate) fn creation_keys(
        &self,
    ) -> Result<Option<super::creation::Validated>, coincube_mcp::Error> {
        if !self.active() {
            return Err(coincube_mcp::Error::AccessDenied);
        }
        self.store
            .creation_input(&self.proposal.proposal_id)?
            .as_ref()
            .map(super::creation::validate)
            .transpose()
    }
    pub(crate) fn unlocked(&self) -> bool {
        crate::app::session::generation_for(&self.cube_id) == Some(self.generation)
    }
    pub(crate) fn active(&self) -> bool {
        self.grant.active()
            && self.proposal.network.as_str() == self.chain.dir_name()
            && crate::app::session::generation_for(&self.cube_id) == Some(self.generation)
    }
    pub(crate) fn invalidate(&self) {
        if let Ok(mut approval) = self.approval.lock() {
            *approval = None;
        }
        let _ = self
            .store
            .native_validation(&self.proposal.proposal_id, false);
    }
    pub(crate) fn close(&self) {
        self.revoke();
        crate::app::session::close_generation(&self.cube_id, self.generation);
    }
    pub(crate) fn revoke(&self) {
        self.grant.revoke();
        self.store.revoke();
        self.invalidate();
    }
    fn validate(&self, ctx: &Context) -> Result<sha256::Hash, Error> {
        if !self.topology().valid()
            || ctx.cube_id.as_deref() != Some(&self.cube_id)
            || self.proposal.network.as_str() != self.chain.dir_name()
            || ctx.bitcoin_config.chain != self.chain
            || ctx.bitcoin_config.check_chain_encoding().is_err()
            || ctx.network != self.chain.bitcoin_network()
            || ctx.use_coincube_connect
            || ctx.backup.is_some()
            || ctx.restored_imported_wallet.is_some()
            || ctx.claim_source.is_some()
            || ctx.keys.values().any(|key| key.provider_key.is_some())
            || ctx.install_node_alongside_connect
            || ctx.pending_bitcoind_config.is_some()
        {
            return Err(failed("Proposal context changed; start a fresh proposal."));
        }
        let descriptor = ctx
            .descriptor
            .as_ref()
            .ok_or_else(|| failed("Select actual keys in Tenshu first."))?;
        if let Some(input) = self
            .store
            .creation_input(&self.proposal.proposal_id)
            .map_err(|_| failed("Wallet preparation is unavailable."))?
        {
            let prepared = super::creation::validate(&input)
                .map_err(|_| failed("Wallet preparation public keys are invalid."))?;
            if *descriptor != prepared.descriptor {
                return Err(failed("Public keys differ from the assistant's immutable proposal. Prepare a fresh proposal before creating."));
            }
        }
        if descriptor.is_taproot() || !descriptor.all_xpubs_net_is(self.chain.bitcoin_network()) {
            return Err(failed(
                "This proposal requires P2WSH keys encoded for its chain.",
            ));
        }
        // CoincubeDescriptor is engine-validated. Read its actual compiled
        // policy, never infer it from requested slots or an AI assertion.
        let policy = descriptor.policy();
        let shape = |path: &PathInfo| match path {
            PathInfo::Single(_) => (1, 1),
            PathInfo::Multi(k, keys) => (*k, keys.len()),
        };
        let Quorum {
            threshold,
            key_count,
        } = self.topology().recovery.quorum;
        let recovery = policy.recovery_paths();
        if shape(policy.primary_path()) != (3, 5)
            || recovery.len() != 1
            || recovery
                .get(&self.topology().recovery.relative_delay_blocks)
                .is_none_or(|path| shape(path) != (usize::from(threshold), usize::from(key_count)))
        {
            return Err(failed("The actual policy differs from the proposal. Return to key selection or prepare a new proposal."));
        }
        let Some(coincubed::config::BitcoinBackend::Bitcoind(_)) = &ctx.bitcoin_backend else {
            return Err(failed("Select a Bitcoin node in native setup first."));
        };
        if self.chain.is_blake2b() {
            // Classify the already configured managed node without creating a
            // wallet directory. Installation still performs the asynchronous
            // beta/feature and exact-chain admission before daemon writes.
            let cfg = coincubed::config::Config::new(
                ctx.bitcoin_config.clone(),
                ctx.bitcoin_backend.clone(),
                log::LevelFilter::Info,
                descriptor.clone(),
                coincubed::datadir::DataDirectory::new(
                    ctx.coincube_directory
                        .network_directory(self.chain)
                        .path()
                        .to_owned(),
                ),
            );
            if !ctx.in_cube_fork_vault
                || !crate::chain::is_managed_local_fork(&cfg, &ctx.coincube_directory)
            {
                return Err(failed("Select Tenshu's managed Bitcoin Blake2b node. Native installation must verify its availability and exact chain."));
            }
        }
        // Fingerprints/provenance/aliases and exact persisted daemon config
        // are bound locally, including auth changes. The digest is private and
        // is not a signature or proof of physical key/backup verification.
        let mut keys = ctx.keys.iter().collect::<Vec<_>>();
        keys.sort_by_key(|(fp, _)| **fp);
        let bytes = serde_json::to_vec(&(
            (self.chain, self.proposal.network),
            descriptor.to_string(),
            ctx.coincube_directory
                .path()
                .canonicalize()
                .map_err(|_| failed("Target directory unavailable"))?,
            keys,
            &ctx.hws
                .iter()
                .map(|(kind, fp, token)| (format!("{kind:?}"), fp, token))
                .collect::<Vec<_>>(),
            &ctx.connect_vault_members
                .iter()
                .map(|m| {
                    (
                        m.fingerprint,
                        m.key_id,
                        m.contact_id,
                        format!("{:?}", m.path_kind),
                    )
                })
                .collect::<Vec<_>>(),
            &ctx.wallet_alias,
            ctx.keychain_keys_recorded,
            &ctx.bitcoin_config,
            &ctx.bitcoin_backend,
            &ctx.pending_bitcoind_config,
            ctx.install_node_alongside_connect,
            (ctx.internal_bitcoind.is_some(), ctx.in_cube_fork_vault),
        ))
        .map_err(|_| failed("Could not bind the native review."))?;
        Ok(sha256::Hash::hash(&bytes))
    }
    fn approve(&self, ctx: &Context) -> bool {
        if !self.active() {
            return false;
        }
        let Ok(snapshot) = self.validate(ctx) else {
            return false;
        };
        let Ok(mut approval) = self.approval.lock() else {
            return false;
        };
        *approval = Some(snapshot);
        true
    }
    fn admit(&self, ctx: &Context, marker: &mut File) -> Result<(), Error> {
        self.grant.admit_native(|| {
            crate::app::session::admit_generation(&self.cube_id, self.generation, || {
                let snapshot = self.validate(ctx).ok()?;
                let mut approval = self.approval.lock().ok()?;
                if approval.take() != Some(snapshot) { return None; }
                // Persistent tombstone prevents retries/other native entrypoints
                // until the locally admitted outcome is inspected. Never delete
                // a possibly partial wallet automatically on an install error.
                marker.write_all(b"Native creation admission attempted. Inspect outcome before retry.\n").ok()?;
                marker.sync_all().ok()?;
                Some(())
            }).ok_or(coincube_mcp::Error::AccessDenied)
        }).map_err(|_| failed("Proposal expired, revoked, changed or already consumed. No new installation was admitted."))?;
        self.store.revoke();
        Ok(())
    }
}
fn failed(message: &str) -> Error {
    Error::Unexpected(message.to_owned())
}

/// OS lock held across durable install AND the native post-install handoff.
/// Shared by native and proposal local installs for a known Cube.
pub(crate) struct Lease {
    _file: File,
    _claim: ProcessClaim,
}
fn registry() -> &'static Mutex<std::collections::HashSet<std::path::PathBuf>> {
    static REGISTRY: OnceLock<Mutex<std::collections::HashSet<std::path::PathBuf>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
struct ProcessClaim(std::path::PathBuf);
impl ProcessClaim {
    fn acquire(path: std::path::PathBuf) -> Result<Self, Error> {
        if !registry()
            .lock()
            .map_err(|_| failed("Creation reservation unavailable"))?
            .insert(path.clone())
        {
            return Err(failed("A Vault creation is already running for this Cube."));
        }
        Ok(Self(path))
    }
}
impl Drop for ProcessClaim {
    fn drop(&mut self) {
        if let Ok(mut registry) = registry().lock() {
            registry.remove(&self.0);
        }
    }
}
pub(crate) fn reserve(ctx: &Context) -> Result<Option<Lease>, Error> {
    let Some(cube) = ctx.cube_id.as_deref() else {
        return Ok(None);
    };
    let key = sha256::Hash::hash(cube.as_bytes());
    let path = ctx
        .coincube_directory
        .path()
        .canonicalize()
        .map_err(|_| failed("Target directory unavailable"))?
        .join(format!(".vault-creation-{key}.lock"));
    let claim = ProcessClaim::acquire(path.clone())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| failed("Could not reserve this Cube for Vault creation."))?;
    if !fs4::fs_std::FileExt::try_lock_exclusive(&file)
        .map_err(|_| failed("Could not lock this Cube's creation reservation."))?
    {
        return Err(failed("A Vault creation is already running for this Cube."));
    }
    if file
        .metadata()
        .map_err(|_| failed("Could not inspect the creation reservation."))?
        .len()
        != 0
    {
        return Err(failed("A prior proposal creation was admitted. Inspect its local outcome before retrying; no automatic retry is allowed."));
    }
    if let Some(proposal) = &ctx.mcp_proposal {
        // Settings must exist for the unlocked target. An unreadable/missing
        // target is not an empty Cube. Run under the same cross-process lock.
        let settings = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory
                .network_directory(ctx.bitcoin_config.chain),
        )
        .map_err(|_| failed("Could not verify the empty target Cube."))?;
        let target = settings
            .cubes
            .iter()
            .find(|c| c.id == cube)
            .ok_or_else(|| failed("Target Cube is unavailable."))?;
        if target.network != proposal.chain
            || target.vault_wallet_id.is_some()
            || target.bitcoin_import.is_some()
        {
            return Err(failed(
                "Target Cube is no longer empty or its chain changed.",
            ));
        }
        proposal.admit(ctx, &mut file)?;
    }
    Ok(Some(Lease {
        _file: file,
        _claim: claim,
    }))
}

/// Native final review immediately before Final's automatic installation task.
pub(crate) struct Review {
    session: Arc<Session>,
    snapshot: Option<sha256::Hash>,
    descriptor: String,
    backend: String,
    signers: Vec<String>,
    warning: Option<String>,
}
impl Review {
    pub(crate) fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            snapshot: None,
            descriptor: String::new(),
            backend: String::new(),
            signers: Vec::new(),
            warning: None,
        }
    }
}
fn account_identity(
    key: &coincube_core::miniscript::descriptor::DescriptorPublicKey,
) -> (
    Option<coincube_core::miniscript::bitcoin::bip32::Fingerprint>,
    Option<coincube_core::miniscript::bitcoin::bip32::Fingerprint>,
) {
    match key {
        coincube_core::miniscript::descriptor::DescriptorPublicKey::XPub(key) => (
            key.origin.as_ref().map(|(fingerprint, _)| *fingerprint),
            Some(key.xkey.fingerprint()),
        ),
        coincube_core::miniscript::descriptor::DescriptorPublicKey::MultiXPub(key) => (
            key.origin.as_ref().map(|(fingerprint, _)| *fingerprint),
            Some(key.xkey.fingerprint()),
        ),
        _ => (None, None),
    }
}

impl Step for Review {
    fn load_context(&mut self, ctx: &Context) {
        self.session.invalidate();
        self.descriptor = ctx
            .descriptor
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        self.backend = match &ctx.bitcoin_backend {
            Some(coincubed::config::BitcoinBackend::Bitcoind(node)) => {
                format!("Bitcoin node · {}", node.addr)
            }
            _ => "Bitcoin node not selected".into(),
        };
        self.signers.clear();
        let assistant_origins = self
            .session
            .creation_keys()
            .ok()
            .flatten()
            .map(|validated| {
                validated
                    .primary
                    .iter()
                    .chain(validated.recovery.iter())
                    .map(|key| key.fingerprint)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(descriptor) = &ctx.descriptor {
            let policy = descriptor.policy();
            let paths = std::iter::once(("Primary".to_owned(), policy.primary_path())).chain(
                policy
                    .recovery_paths()
                    .iter()
                    .map(|(delay, path)| (format!("Recovery after {delay} blocks"), path)),
            );
            for (path_name, path) in paths {
                let mut descriptor_keys = match path {
                    coincube_core::descriptors::PathInfo::Single(key) => vec![key.clone()],
                    coincube_core::descriptors::PathInfo::Multi(_, keys) => keys.clone(),
                };
                descriptor_keys.sort_by_key(|key| account_identity(key).0);
                for desc_key in descriptor_keys {
                    let (origin, account) = account_identity(&desc_key);
                    let Some(fp) = origin else {
                        continue;
                    };
                    let key = ctx.keys.get(&fp);
                    let alias = key
                        .map(|k| k.name.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Unnamed signer");
                    let assistant_supplied = assistant_origins.contains(&fp);
                    let (identity, source) = if assistant_supplied {
                        (
                            account
                                .map(|fingerprint| {
                                    format!("account xpub fingerprint {fingerprint}")
                                })
                                .unwrap_or_else(|| {
                                    "account xpub fingerprint unavailable".to_string()
                                }),
                            "origin fingerprint was supplied by the assistant and was not checked",
                        )
                    } else if key.is_some_and(|k| k.keychain_key_id.is_some()) {
                        (
                            format!("fingerprint {fp}"),
                            "Keychain metadata recorded; verify on the phone",
                        )
                    } else if ctx.hws.iter().any(|(_, fingerprint, _)| *fingerprint == fp) {
                        (
                            format!("fingerprint {fp}"),
                            "hardware registration recorded; verify on the device",
                        )
                    } else if key.is_some_and(|k| k.is_border_wallet) {
                        (
                            format!("fingerprint {fp}"),
                            "Border Wallet provenance recorded; verify its source",
                        )
                    } else {
                        (
                            format!("fingerprint {fp}"),
                            "verify the original key source locally",
                        )
                    };
                    self.signers
                        .push(format!("{path_name}: {alias} · {identity} · {source}"));
                }
            }
        }
        match self.session.validate(ctx) {
            Ok(snapshot) if self.session.active() => {
                self.snapshot = Some(snapshot);
                self.warning = None;
                let _ = self
                    .session
                    .store
                    .native_validation(&self.session.proposal.proposal_id, true);
            }
            _ => {
                self.snapshot = None;
                self.warning = Some("Proposal inactive or policy/backend changed. Return to key selection or reopen the Cube and prepare a new proposal.".into());
            }
        }
    }
    fn apply(&mut self, ctx: &mut Context) -> bool {
        self.snapshot
            .is_some_and(|hash| self.session.validate(ctx).ok() == Some(hash))
            && self.session.approve(ctx)
    }
    fn view<'a>(
        &'a self,
        _hws: &'a crate::hw::HardwareWallets,
        _progress: (usize, usize),
    ) -> Element<'a, Message> {
        let topology = self.session.topology();
        let mut content = column![h3(localized_copy("Review and authorize this Vault")),
            text(format!("Network · {}", self.session.chain().dir_name())),
            text(&self.backend),
            text(coincube_ui::localized_format!("Primary: 3 of 5. Recovery: {} of {} after {} relative blocks for each coin. Block time varies; this is not a calendar deadline.", "Principal: 3 de 5. Recuperación: {} de {} después de {} bloques relativos para cada moneda. El tiempo de los bloques varía; no es una fecha límite de calendario.", topology.recovery.quorum.threshold, topology.recovery.quorum.key_count, topology.recovery.relative_delay_blocks)),
            text(localized_copy("Tenshu validated this actual descriptor. For assistant-supplied keys, the account xpub fingerprint below is computed from the key itself; the origin fingerprint was supplied by the assistant and was not checked. Verify every key on its source device, signer distribution and recovery assumptions before creating. Distinct fingerprints do not prove independent custody.")),
            text(&self.descriptor),
            text(localized_copy("Creation ends MCP access. A separate approval is required to read the resulting Vault. Backup possession and recovery testing are not verified by descriptor validation.")),
            text(localized_copy(self.warning.as_deref().unwrap_or("Confirm that this is the exact policy and set of public keys you intend.")))].spacing(16).padding(24);
        for signer in &self.signers {
            content = content.push(text(signer));
        }
        content = content
            .push(button(localized_copy("Back to setup")).on_press(Message::Previous))
            .push(button(localized_copy("Create this Vault")).on_press_maybe(
                (self.snapshot.is_some() && self.session.active()).then_some(Message::Next),
            ));
        scrollable(content).into()
    }
    fn revert(&self, _ctx: &mut Context) {
        self.session.invalidate();
    }
    fn load(&self) -> Task<Message> {
        Task::none()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use coincube_core::{
        descriptors::{CoincubeDescriptor, CoincubePolicy},
        miniscript::{
            bitcoin::{
                bip32::{Xpriv, Xpub},
                secp256k1::Secp256k1,
                Network,
            },
            descriptor::DescriptorPublicKey,
        },
    };
    use coincube_mcp::proposal::{PrepareInput, Recovery};
    use std::{collections::BTreeMap, str::FromStr, time::Duration};
    fn topology() -> Topology {
        Topology {
            primary: Quorum {
                threshold: 3,
                key_count: 5,
            },
            recovery: Recovery {
                quorum: Quorum {
                    threshold: 2,
                    key_count: 3,
                },
                relative_delay_blocks: 144,
            },
        }
    }
    fn descriptor(k: usize) -> CoincubeDescriptor {
        let secp = Secp256k1::new();
        let keys: Vec<_> = (1..=8)
            .map(|i| {
                let private = Xpriv::new_master(Network::Regtest, &[i; 32]).unwrap();
                let public = Xpub::from_priv(&secp, &private);
                DescriptorPublicKey::from_str(&format!(
                    "[{}]{public}/<0;1>/*",
                    public.fingerprint()
                ))
                .unwrap()
            })
            .collect();
        CoincubeDescriptor::new(
            CoincubePolicy::new_legacy(
                PathInfo::Multi(k, keys[..5].to_vec()),
                BTreeMap::from([(144, PathInfo::Multi(2, keys[5..].to_vec()))]),
            )
            .unwrap(),
        )
    }
    pub(crate) struct TestDir(std::path::PathBuf);
    impl TestDir {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("mcp-proposal-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        pub(crate) fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn fixture(root: &std::path::Path) -> (Context, Arc<Session>) {
        fixture_for_chain(root, crate::chain::ChainId::Regtest)
    }
    /// Native installer callers need an authenticated root as well as a proposal.
    /// Keep the authless fixture for protocol-only authorization tests.
    pub(crate) fn authenticated_fixture_for_chain(
        root: &std::path::Path,
        chain: crate::chain::ChainId,
    ) -> (Context, Arc<Session>) {
        let (ctx, session) = fixture_for_chain(root, chain);
        let dir = ctx.coincube_directory.network_directory(chain);
        let mut settings = crate::app::settings::Settings::from_file(&dir).unwrap();
        let master =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let fp = master.fingerprint(&Secp256k1::new());
        let cube = &mut settings.cubes[0];
        let generation = crate::app::session::generation_for(&cube.id);
        cube.master_signer_fingerprint = Some(fp);
        crate::app::session::store_unlocked_signer(&cube.id, fp, master);
        assert_eq!(crate::app::session::generation_for(&cube.id), generation);
        assert!(session.active());
        std::fs::write(
            dir.path().join("settings.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        (ctx, session)
    }

    pub(crate) fn fixture_for_chain(
        root: &std::path::Path,
        chain: crate::chain::ChainId,
    ) -> (Context, Arc<Session>) {
        let network = chain.dir_name().parse::<coincube_mcp::Chain>().unwrap();
        let cube = crate::app::settings::CubeSettings::new_with_raw_id(
            "proposal-test".into(),
            "Disposable".into(),
            chain,
        );
        crate::app::session::open_without_pin(&cube.id);
        let generation = crate::app::session::generation_for(&cube.id).unwrap();
        let mut ctx = Context::new_for_chain(
            chain,
            crate::dir::CoincubeDirectory::new(root.to_owned()),
            Some(&cube),
            None,
        );
        let store = Arc::new(Store::for_chain(network));
        let proposal = store
            .prepare(PrepareInput {
                request_id: "fixture".into(),
                network,
                topology: topology(),
            })
            .unwrap();
        let grant = Arc::new(Grant::for_proposals(Duration::from_secs(60), || true));
        let session = Arc::new(Session::new(
            cube.id.clone(),
            generation,
            chain,
            grant,
            store,
            proposal,
        ));
        ctx.mcp_proposal = Some(session.clone());
        ctx.in_cube_fork_vault = chain.is_blake2b();
        ctx.descriptor = Some(
            super::super::creation::validate(&super::super::creation::tests::input_for_chain(
                network,
            ))
            .unwrap()
            .descriptor,
        );
        ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Bitcoind(
            coincubed::config::BitcoindConfig {
                addr: "127.0.0.1:28443".parse().unwrap(),
                rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                    "fixture".into(),
                    "not-a-secret".into(),
                ),
            },
        ));
        let dir = root.join(chain.dir_name());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&crate::app::settings::Settings {
                cubes: vec![cube],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        (ctx, session)
    }
    #[test]
    fn exact_native_review_and_admission_do_not_install_or_export_keys() {
        let _guard = crate::app::session::test_guard();
        let temp = TestDir::new();
        let (ctx, session) = fixture(temp.path());
        assert!(session.validate(&ctx).is_ok());
        assert!(
            !temp.path().join("regtest/data").exists(),
            "validation cannot create wallet directories"
        );
        assert!(
            reserve(&ctx).is_err(),
            "a prepared/valid descriptor is not native authorization"
        );
        assert!(session.grant.active());
        let mut review = Review::new(session.clone());
        review.load_context(&ctx);
        let projected = session
            .store
            .get(&coincube_mcp::proposal::ProposalInput {
                proposal_id: session.proposal.proposal_id.clone(),
            })
            .unwrap();
        assert!(projected.descriptor_validated);
        let json = serde_json::to_string(&projected).unwrap();
        assert!(!json.contains("tpub"));
        assert!(!json.contains("not-a-secret"));
        assert!(session.approve(&ctx));
        let lease = reserve(&ctx).unwrap().unwrap();
        assert!(!session.grant.active());
        assert!(session.store.pending().is_err());
        assert!(
            reserve(&ctx).is_err(),
            "shared OS reservation blocks duplicate work"
        );
        drop(lease);
        assert!(
            reserve(&ctx).is_err(),
            "durable admission tombstone blocks replay even after process lease ends"
        );
        assert!(
            !temp.path().join("regtest/data").exists(),
            "admission test does not perform an actual install"
        );
        crate::app::session::close();
    }
    #[test]
    fn edits_back_navigation_and_session_reopen_invalidate_native_authority() {
        let _guard = crate::app::session::test_guard();
        let temp = TestDir::new();
        let (mut ctx, session) = fixture(temp.path());
        assert!(session.approve(&ctx));
        ctx.wallet_alias = "changed after approval".into();
        assert!(reserve(&ctx).is_err());
        assert!(session.grant.active());
        assert!(session.approve(&ctx));
        session.invalidate();
        assert!(reserve(&ctx).is_err());
        assert!(session.approve(&ctx));
        ctx.descriptor = Some(descriptor(2));
        assert!(session.validate(&ctx).is_err());
        assert!(reserve(&ctx).is_err());
        ctx.descriptor = Some(descriptor(3));
        assert!(session.approve(&ctx));
        crate::app::session::close();
        crate::app::session::open_without_pin("proposal-test");
        assert!(
            reserve(&ctx).is_err(),
            "same Cube with a new generation cannot revive approval"
        );
        crate::app::session::close();
    }
    #[test]
    fn production_native_review_binds_chain_backend_and_target_without_installing() {
        let _guard = crate::app::session::test_guard();
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::Testnet4,
        ] {
            let temp = TestDir::new();
            let (mut ctx, session) = fixture_for_chain(temp.path(), chain);
            assert!(session.approve(&ctx));
            // A backend change invalidates the exact native approval, even
            // though it remains a valid selection for a subsequent review.
            if let Some(coincubed::config::BitcoinBackend::Bitcoind(node)) =
                &mut ctx.bitcoin_backend
            {
                node.addr.set_port(8332);
            }
            assert!(reserve(&ctx).is_err());
            assert!(session.approve(&ctx));
            let lease = reserve(&ctx).unwrap();
            assert!(!session.active());
            assert!(!temp.path().join(chain.dir_name()).join("data").exists());
            drop(lease);
            crate::app::session::close();
        }
    }

    #[test]
    fn production_review_requires_managed_fork_backend_and_keeps_fork_identity() {
        let _guard = crate::app::session::test_guard();
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let temp = TestDir::new();
            let (mut ctx, session) = fixture_for_chain(temp.path(), chain);
            assert!(ctx.in_cube_fork_vault);
            assert!(
                !session.approve(&ctx),
                "arbitrary RPC cannot bypass fork admission"
            );
            let directory = crate::node::bitcoind::internal_bitcoind_datadir_for(
                &ctx.coincube_directory,
                crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
            );
            std::fs::create_dir_all(&directory).unwrap();
            let mut managed = crate::node::bitcoind::InternalBitcoindConfig::for_flavor(
                crate::node::bitcoind::NodeFlavor::KnotsBlake2b,
            );
            managed.networks.insert(
                chain.bitcoin_network(),
                crate::node::bitcoind::InternalBitcoindNetworkConfig {
                    rpc_port: 18444,
                    p2p_port: 18445,
                    prune: 550,
                    rpc_auth: None,
                },
            );
            managed
                .to_ini()
                .write_to_file(crate::node::bitcoind::internal_bitcoind_config_path(
                    &directory,
                ))
                .unwrap();
            ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Bitcoind(
                coincubed::config::BitcoindConfig {
                    addr: "127.0.0.1:18444".parse().unwrap(),
                    rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                        crate::node::bitcoind::internal_bitcoind_cookie_path(
                            &directory,
                            &chain.bitcoin_network(),
                        ),
                    ),
                },
            ));
            assert!(session.approve(&ctx));
            // Identical address/xpub encoding cannot stand in for exact chain.
            ctx.bitcoin_config.chain = crate::chain::ChainId::from(chain.bitcoin_network());
            assert!(session.validate(&ctx).is_err());
            assert!(reserve(&ctx).is_err());
            ctx.bitcoin_config.chain = chain;
            assert!(session.approve(&ctx));
            let lease = reserve(&ctx).unwrap();
            assert!(!session.active());
            assert!(!temp.path().join(chain.dir_name()).join("data").exists());
            drop(lease);
            crate::app::session::close();
        }
    }

    #[test]
    fn read_grants_revocation_wrong_target_or_backend_cannot_create() {
        let _guard = crate::app::session::test_guard();
        let temp = TestDir::new();
        let (mut ctx, session) = fixture(temp.path());
        assert!(session.approve(&ctx));
        ctx.bitcoin_config = coincubed::config::BitcoinConfig::new(
            crate::chain::ChainId::Bitcoin,
            Duration::from_secs(10),
        );
        assert!(session.validate(&ctx).is_err());
        assert!(reserve(&ctx).is_err());
        ctx.bitcoin_config = coincubed::config::BitcoinConfig::new(
            crate::chain::ChainId::Regtest,
            Duration::from_secs(10),
        );
        ctx.bitcoin_backend = None;
        assert!(session.validate(&ctx).is_err());
        assert!(reserve(&ctx).is_err());
        session.revoke();
        assert!(!session.approve(&ctx));
        assert!(reserve(&ctx).is_err());
        assert!(!temp.path().join("regtest/data").exists());
        crate::app::session::close();
    }
    #[test]
    fn normal_native_and_proposal_paths_share_the_cube_reservation() {
        let _guard = crate::app::session::test_guard();
        let temp = TestDir::new();
        let (ctx, session) = fixture(temp.path());
        let mut normal = ctx.clone();
        normal.mcp_proposal = None;
        let native = reserve(&normal).unwrap();
        assert!(session.approve(&ctx));
        assert!(reserve(&ctx).is_err());
        drop(native);
        let proposed = reserve(&ctx).unwrap();
        assert!(reserve(&normal).is_err());
        drop(proposed);
        assert!(reserve(&normal).is_err());
        crate::app::session::close();
    }
    #[test]
    fn cube_reservation_excludes_another_process() {
        let _guard = crate::app::session::test_guard();
        let temp = TestDir::new();
        let (mut ctx, _) = fixture(temp.path());
        ctx.mcp_proposal = None;
        let _lease = reserve(&ctx).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "installer::proposal::tests::os_cube_reservation_child",
                "--nocapture",
            ])
            .env("TENSHU_PROPOSAL_LOCK_TEST_ROOT", temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child reservation check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        crate::app::session::close();
    }
    #[test]
    fn os_cube_reservation_child() {
        let Some(root) = std::env::var_os("TENSHU_PROPOSAL_LOCK_TEST_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("mcp-proposal-"));
        let mut ctx = Context::new(
            coincube_core::miniscript::bitcoin::Network::Regtest,
            crate::dir::CoincubeDirectory::new(root),
            None,
            None,
        );
        ctx.cube_id = Some("proposal-test".into());
        assert!(
            reserve(&ctx).is_err(),
            "OS lock must block the independent child process"
        );
    }
}
