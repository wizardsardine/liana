//! Recovery Kit metadata copied at explicit wallet/settings boundaries.
//! Rendering and ordinary update synchronization never read settings files.
use std::path::PathBuf;

use super::settings::{
    bitcoin_import::ImportedClaimPair, CubeSettings, ImportedWalletAttachment, Settings,
    WalletSettings,
};
use crate::{chain::ChainId, dir::CoincubeDirectory};

#[derive(Clone, PartialEq, Eq)]
struct AttachmentIdentity {
    root: PathBuf,
    cube: String,
    chain: ChainId,
    operation: uuid::Uuid,
    wallet: ImportedWalletAttachment,
    pair: Option<Box<ImportedClaimPair>>,
}
impl AttachmentIdentity {
    fn of(root: &CoincubeDirectory, cube: &CubeSettings) -> Option<Self> {
        if cube.vault_wallet_id.is_some() {
            return None;
        }
        let import = cube.bitcoin_import.as_ref()?;
        let wallet = import.active_attachment()?;
        wallet.validate(cube.network).ok()?;
        Some(Self {
            root: root.path().to_path_buf(),
            cube: cube.id.clone(),
            chain: cube.network,
            operation: import.operation_id,
            wallet: wallet.clone(),
            pair: cube.imported_claim_source.clone(),
        })
    }
}

#[derive(Default)]
pub(super) struct ImportedKitMetadata {
    identity: Option<Box<AttachmentIdentity>>,
    metadata: Option<Box<WalletSettings>>,
}
impl ImportedKitMetadata {
    /// A known wallet/settings change makes the current fingerprint unknown
    /// until the corresponding durable metadata has been loaded successfully.
    pub(super) fn invalidate(&mut self) {
        self.identity = None;
        self.metadata = None;
    }

    /// Used only at open/adoption or a completed wallet metadata update.
    pub(super) fn reload(&mut self, root: &CoincubeDirectory, cube: &CubeSettings) {
        self.invalidate();
        if AttachmentIdentity::of(root, cube).is_none() {
            return;
        }
        if let Ok(settings) = Settings::from_file(&root.network_directory(cube.network)) {
            self.replace(root, cube, &settings);
        }
    }

    /// Reuse a settings snapshot already read by an import/save boundary.
    /// Missing, ambiguous or mismatched records leave the fingerprint unknown.
    pub(super) fn replace(
        &mut self,
        root: &CoincubeDirectory,
        cube: &CubeSettings,
        settings: &Settings,
    ) {
        self.invalidate();
        let Some(identity) = AttachmentIdentity::of(root, cube) else {
            return;
        };
        let mut cubes = settings.cubes.iter().filter(|saved| saved.id == cube.id);
        let Some(saved_cube) = cubes.next() else {
            return;
        };
        if cubes.next().is_some()
            || AttachmentIdentity::of(root, saved_cube).as_ref() != Some(&identity)
        {
            return;
        }
        let mut wallets = settings
            .wallets
            .iter()
            .filter(|wallet| wallet.wallet_id() == identity.wallet.wallet_id);
        let Some(metadata) = wallets.next() else {
            return;
        };
        if wallets.next().is_some() {
            return;
        }
        let mut metadata = metadata.clone();
        // Match encrypted_signer_metadata: account credentials and local
        // node/rescan preferences never enter the encrypted wallet payload.
        metadata.remote_backend_auth = None;
        metadata.start_internal_bitcoind = Some(false);
        metadata.pending_rescan = None;
        self.identity = Some(Box::new(identity));
        self.metadata = Some(Box::new(metadata));
    }

    /// Pure synchronization: use current verified allocation indices and the
    /// full cached metadata. A changed attachment can never reuse old metadata,
    /// including if the caller later switches back to the previous attachment.
    pub(super) fn fingerprint(
        &mut self,
        root: &CoincubeDirectory,
        cube: &CubeSettings,
        chain: ChainId,
        address_state: Option<(u32, u32)>,
    ) -> Option<String> {
        let identity = AttachmentIdentity::of(root, cube);
        if chain != cube.network || identity.as_ref() != self.identity.as_deref() {
            self.invalidate();
            return None;
        }
        let identity = identity?;
        let metadata = self.metadata.as_deref()?;
        let (receive_index, change_index) = address_state?;
        let blob = crate::services::recovery::ImportedDescriptorBlob::new_with_pair(
            cube.id.clone(),
            chain,
            crate::services::recovery::ImportedDescriptorBlobWallet {
                name: cube.name.clone(),
                descriptor: identity.wallet.descriptor,
                birthday: identity.wallet.birthday,
                initial_lookahead: identity.wallet.initial_lookahead,
                receive_index,
                change_index,
            },
            identity.pair,
            Some(metadata.clone()),
            Some(identity.operation),
        )
        .ok()?;
        crate::services::recovery::imported_descriptor_blob_fingerprint(&blob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{settings, wallet::Wallet, App, Cache, Config, Message};
    use crate::services::recovery::{
        imported::encrypted_signer_metadata, imported_descriptor_blob_fingerprint,
        ImportedDescriptorBlob, ImportedDescriptorBlobWallet,
    };
    use std::sync::Arc;

    struct Fixture {
        root: CoincubeDirectory,
        cube: CubeSettings,
        settings: Settings,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root.path());
        }
    }
    impl Fixture {
        fn new() -> Self {
            let cube = crate::services::bitcoin_import::pairing::tests::imported_cube();
            let wallet = cube
                .bitcoin_import
                .as_ref()
                .unwrap()
                .active_attachment()
                .unwrap();
            let metadata = WalletSettings {
                name: "Synthetic wallet metadata".into(),
                alias: Some("Synthetic alias".into()),
                descriptor_checksum: wallet.wallet_id.descriptor_checksum.clone(),
                pinned_at: wallet.wallet_id.timestamp,
                keys: vec![settings::KeySetting {
                    name: "Synthetic external signer".into(),
                    master_fingerprint: [1, 2, 3, 4].into(),
                    provider_key: None,
                    is_border_wallet: false,
                    grid_seed_source: None,
                    replay_protected: None,
                    keychain_key_id: Some(42),
                }],
                hardware_wallets: Vec::new(),
                remote_backend_auth: Some(settings::AuthConfig::new(
                    "synthetic@example.invalid".into(),
                    "synthetic-account".into(),
                )),
                start_internal_bitcoind: Some(true),
                pending_rescan: Some(settings::PendingRescan::DateUnknown),
                keychain_keys_recorded: true,
            };
            let fixture = Self {
                root: CoincubeDirectory::new(
                    std::env::temp_dir()
                        .join(format!("imported-kit-cache-{}", uuid::Uuid::new_v4())),
                ),
                settings: Settings {
                    cubes: vec![cube.clone()],
                    wallets: vec![metadata],
                    ..Default::default()
                },
                cube,
            };
            fixture.save();
            fixture
        }
        fn save(&self) {
            let path = self.path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, serde_json::to_vec(&self.settings).unwrap()).unwrap();
        }
        fn path(&self) -> PathBuf {
            self.root
                .network_directory(self.cube.network)
                .path()
                .join(settings::SETTINGS_FILE_NAME)
        }
        fn fingerprint(&self, cache: &mut ImportedKitMetadata) -> Option<String> {
            cache.fingerprint(&self.root, &self.cube, self.cube.network, Some((7, 3)))
        }
        fn expected(&self, receive_index: u32, change_index: u32) -> Option<String> {
            let import = self.cube.bitcoin_import.as_ref().unwrap();
            let wallet = import.active_attachment().unwrap();
            let blob = ImportedDescriptorBlob::new_with_pair(
                self.cube.id.clone(),
                self.cube.network,
                ImportedDescriptorBlobWallet {
                    name: self.cube.name.clone(),
                    descriptor: wallet.descriptor.clone(),
                    birthday: wallet.birthday,
                    initial_lookahead: wallet.initial_lookahead,
                    receive_index,
                    change_index,
                },
                self.cube.imported_claim_source.clone(),
                Some(
                    encrypted_signer_metadata(
                        &self.root.network_directory(self.cube.network),
                        &wallet.wallet_id,
                    )
                    .unwrap(),
                ),
                Some(import.operation_id),
            )
            .unwrap();
            imported_descriptor_blob_fingerprint(&blob)
        }
        fn app(&self) -> App {
            use crate::daemon::embedded::EmbeddedDaemon;
            use coincubed::config::{BitcoinBackend, BitcoinConfig, ElectrumConfig};
            let wallet = Arc::new(Wallet::from_imported_cube(&self.cube).unwrap());
            let attachment = self
                .cube
                .bitcoin_import
                .as_ref()
                .unwrap()
                .active_attachment()
                .unwrap();
            let config = coincubed::config::Config::new(
                BitcoinConfig::new(self.cube.network, std::time::Duration::from_secs(30)),
                Some(BitcoinBackend::Electrum(ElectrumConfig {
                    addr: "tcp://127.0.0.1:1".into(),
                    validate_domain: false,
                })),
                log::LevelFilter::Off,
                attachment.descriptor.clone(),
                self.root
                    .network_directory(self.cube.network)
                    .coincubed_data_directory(&attachment.wallet_id),
            );
            let (app, startup) = App::new_inner(
                Cache {
                    network: self.cube.network.bitcoin_network(),
                    fiat_chain: self.cube.network,
                    datadir_path: self.root.clone(),
                    ..Default::default()
                },
                wallet,
                None,
                None,
                Config::new(false),
                Arc::new(EmbeddedDaemon::unstarted_for_test(config, None)),
                self.root.clone(),
                None,
                self.cube.clone(),
                None,
            );
            drop(startup);
            app
        }
    }

    #[test]
    fn imported_kit_cache_matches_full_fresh_backup_and_avoids_passive_disk_reads() {
        let fixture = Fixture::new();
        let mut cache = ImportedKitMetadata::default();
        cache.reload(&fixture.root, &fixture.cube);
        let expected = fixture.expected(7, 3);
        assert!(expected.is_some());
        assert_eq!(fixture.fingerprint(&mut cache), expected);
        assert!(cache
            .metadata
            .as_ref()
            .unwrap()
            .remote_backend_auth
            .is_none());
        assert_eq!(
            cache.metadata.as_ref().unwrap().start_internal_bitcoind,
            Some(false)
        );
        assert!(cache.metadata.as_ref().unwrap().pending_rescan.is_none());
        // A passive sync cannot even parse this file; cached metadata remains
        // available until an explicit metadata/attachment invalidation.
        std::fs::write(fixture.path(), b"synthetic-invalid-settings").unwrap();
        for _ in 0..20 {
            assert_eq!(fixture.fingerprint(&mut cache), expected);
        }
        assert!(cache
            .fingerprint(&fixture.root, &fixture.cube, fixture.cube.network, None)
            .is_none());
        assert_eq!(fixture.fingerprint(&mut cache), expected);
        cache.invalidate();
        assert!(fixture.fingerprint(&mut cache).is_none());
        cache.reload(&fixture.root, &fixture.cube);
        assert!(fixture.fingerprint(&mut cache).is_none());
    }

    #[test]
    fn imported_kit_cache_identity_changes_cannot_reuse_a_prior_healthy_fingerprint() {
        let fixture = Fixture::new();
        for case in ["root", "cube", "chain", "operation", "wallet", "pair"] {
            let mut cache = ImportedKitMetadata::default();
            cache.replace(&fixture.root, &fixture.cube, &fixture.settings);
            assert!(fixture.fingerprint(&mut cache).is_some());
            let mut cube = fixture.cube.clone();
            let mut root = fixture.root.clone();
            match case {
                "root" => root = CoincubeDirectory::new(fixture.root.path().join("another-root")),
                "cube" => cube.id = "another-cube".into(),
                "chain" => cube.network = ChainId::BitcoinBlake2b,
                "operation" => {
                    cube.bitcoin_import.as_mut().unwrap().operation_id = uuid::Uuid::new_v4()
                }
                "wallet" => {
                    let settings::BitcoinImportStage::Ready { wallet } =
                        &mut cube.bitcoin_import.as_mut().unwrap().stage
                    else {
                        unreachable!()
                    };
                    wallet.birthday += 1;
                }
                "pair" => {
                    cube.imported_claim_source = Some(Box::new(ImportedClaimPair {
                        source_cube_id: "another-source".into(),
                        source_operation_id: uuid::Uuid::new_v4(),
                        source_wallet: fixture
                            .cube
                            .bitcoin_import
                            .as_ref()
                            .unwrap()
                            .active_attachment()
                            .unwrap()
                            .clone(),
                    }))
                }
                _ => unreachable!(),
            }
            assert!(
                cache
                    .fingerprint(&root, &cube, cube.network, Some((7, 3)))
                    .is_none(),
                "{}",
                case
            );
            assert!(fixture.fingerprint(&mut cache).is_none(), "{}", case);
        }
    }

    #[test]
    fn imported_kit_cache_missing_ambiguous_and_changed_settings_fail_closed() {
        let fixture = Fixture::new();
        for case in [
            "missing_cube",
            "duplicate_cube",
            "changed_operation",
            "missing_wallet",
            "duplicate_wallet",
            "wrong_wallet",
        ] {
            let mut cache = ImportedKitMetadata::default();
            cache.replace(&fixture.root, &fixture.cube, &fixture.settings);
            assert!(fixture.fingerprint(&mut cache).is_some());
            let mut saved = fixture.settings.clone();
            match case {
                "missing_cube" => saved.cubes.clear(),
                "duplicate_cube" => saved.cubes.push(fixture.cube.clone()),
                "changed_operation" => {
                    saved.cubes[0].bitcoin_import.as_mut().unwrap().operation_id =
                        uuid::Uuid::new_v4()
                }
                "missing_wallet" => saved.wallets.clear(),
                "duplicate_wallet" => saved.wallets.push(saved.wallets[0].clone()),
                "wrong_wallet" => saved.wallets[0].descriptor_checksum = "another-wallet".into(),
                _ => unreachable!(),
            }
            cache.replace(&fixture.root, &fixture.cube, &saved);
            assert!(fixture.fingerprint(&mut cache).is_none(), "{}", case);
        }
    }

    #[test]
    fn imported_kit_cache_keeps_address_operation_and_signer_drift_in_fingerprint() {
        let mut fixture = Fixture::new();
        let mut cache = ImportedKitMetadata::default();
        cache.reload(&fixture.root, &fixture.cube);
        let before = fixture.fingerprint(&mut cache).unwrap();
        let advanced = cache
            .fingerprint(
                &fixture.root,
                &fixture.cube,
                fixture.cube.network,
                Some((8, 3)),
            )
            .unwrap();
        assert_ne!(advanced, before);
        assert_eq!(Some(advanced), fixture.expected(8, 3));
        fixture.settings.wallets[0].keys[0].keychain_key_id = Some(99);
        fixture.save();
        cache.reload(&fixture.root, &fixture.cube);
        let changed_signer = fixture.fingerprint(&mut cache).unwrap();
        assert_ne!(changed_signer, before);
        assert_eq!(Some(changed_signer.clone()), fixture.expected(7, 3));
        fixture.cube.bitcoin_import.as_mut().unwrap().operation_id = uuid::Uuid::new_v4();
        assert!(fixture.fingerprint(&mut cache).is_none());
        fixture.settings.cubes[0] = fixture.cube.clone();
        fixture.save();
        cache.reload(&fixture.root, &fixture.cube);
        assert_ne!(fixture.fingerprint(&mut cache).unwrap(), changed_signer);
        assert_eq!(fixture.fingerprint(&mut cache), fixture.expected(7, 3));
    }

    #[test]
    fn imported_kit_cache_keeps_pair_binding_drift_in_full_fingerprint() {
        let mut fixture = Fixture::new();
        let source = fixture.cube.clone();
        let import = source.bitcoin_import.as_ref().unwrap();
        let source_wallet = import.active_attachment().unwrap();
        let wallet = ImportedWalletAttachment::new(
            source_wallet.descriptor.paired_blake2b().unwrap(),
            ChainId::BitcoinBlake2b,
            source_wallet.birthday,
            source_wallet.initial_lookahead,
        )
        .unwrap();
        let mut paired = CubeSettings::new_with_raw_id(
            crate::services::bitcoin_import::pairing::target_cube_id(&source.id),
            "Synthetic paired wallet".into(),
            ChainId::BitcoinBlake2b,
        );
        paired.master_signer_fingerprint = Some([1, 2, 3, 4].into());
        paired.imported_claim_source = Some(Box::new(ImportedClaimPair {
            source_cube_id: source.id,
            source_operation_id: import.operation_id,
            source_wallet: source_wallet.clone(),
        }));
        fixture.settings.wallets[0].descriptor_checksum =
            wallet.wallet_id.descriptor_checksum.clone();
        paired.bitcoin_import = Some(Box::new(settings::BitcoinImport {
            operation_id: uuid::Uuid::parse_str(&paired.id).unwrap(),
            stage: settings::BitcoinImportStage::Ready { wallet },
            last_error: None,
        }));
        fixture.cube = paired;
        fixture.settings.cubes[0] = fixture.cube.clone();
        fixture.save();
        let mut cache = ImportedKitMetadata::default();
        cache.reload(&fixture.root, &fixture.cube);
        let before = fixture.fingerprint(&mut cache).unwrap();
        assert_eq!(Some(before.clone()), fixture.expected(7, 3));
        fixture
            .cube
            .imported_claim_source
            .as_mut()
            .unwrap()
            .source_operation_id = uuid::Uuid::new_v4();
        assert!(fixture.fingerprint(&mut cache).is_none());
        fixture.settings.cubes[0] = fixture.cube.clone();
        fixture.save();
        cache.reload(&fixture.root, &fixture.cube);
        assert_ne!(fixture.fingerprint(&mut cache).unwrap(), before);
        assert_eq!(fixture.fingerprint(&mut cache), fixture.expected(7, 3));
    }

    #[test]
    fn imported_kit_cache_save_and_wallet_failure_invalidate_at_app_boundary() {
        let mut fixture = Fixture::new();
        let mut app = fixture.app();
        let original = fixture.fingerprint(&mut app.imported_kit_metadata).unwrap();
        std::fs::write(fixture.path(), b"synthetic-invalid-settings").unwrap();
        for _ in 0..3 {
            drop(
                app.update(Message::View(crate::app::view::Message::ShowToast(
                    log::Level::Info,
                    "Synthetic passive event".into(),
                ))),
            );
            assert_eq!(
                fixture.fingerprint(&mut app.imported_kit_metadata).as_ref(),
                Some(&original)
            );
        }
        drop(app.update(Message::SettingsSaved));
        assert!(fixture
            .fingerprint(&mut app.imported_kit_metadata)
            .is_none());
        assert!(app.cache.current_descriptor_fingerprint.is_none());
        fixture.settings.wallets[0].keys[0].name = "Updated synthetic signer".into();
        fixture.save();
        drop(app.update(Message::SettingsSaved));
        let updated = fixture.fingerprint(&mut app.imported_kit_metadata).unwrap();
        assert_ne!(updated, original);
        assert_eq!(Some(updated), fixture.expected(7, 3));
        drop(app.update(Message::WalletUpdated(Err(
            crate::app::error::Error::Unexpected("synthetic wallet update failure".into()),
        ))));
        assert!(fixture
            .fingerprint(&mut app.imported_kit_metadata)
            .is_none());
        assert!(app.cache.current_descriptor_fingerprint.is_none());
    }
}
