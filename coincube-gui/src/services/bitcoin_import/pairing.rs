//! Dedicated imported-wallet fork pairing. The copied Cube seed is only the
//! unlock identity; all spending policy and external signers remain imported.
pub(crate) mod source_lock;

use super::*;
use crate::{
    app::settings::{bitcoin_import::ImportedClaimPair, BitcoinImport, CubeSettings},
    chain::ChainId,
    dir::CoincubeDirectory,
    signer::Signer,
};
use sha2::{Digest, Sha256};

pub fn target_cube_id(source_cube_id: &str) -> String {
    let digest =
        Sha256::digest(format!("coincube/imported-btcb2-pair/v1/{source_cube_id}").as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes).to_string()
}

fn unique_cube<'a>(saved: &'a Settings, id: &str) -> Result<&'a CubeSettings, String> {
    let mut matches = saved.cubes.iter().filter(|c| c.id == id);
    let cube = matches
        .next()
        .ok_or("The paired Cube is no longer available.")?;
    if matches.next().is_some() {
        return Err("Duplicate Cube records prevent safe pairing.".into());
    }
    Ok(cube)
}

fn unique_wallet<'a>(saved: &'a Settings, id: &WalletId) -> Result<&'a WalletSettings, String> {
    let mut matches = saved.wallets.iter().filter(|w| &w.wallet_id() == id);
    let wallet = matches
        .next()
        .ok_or("Imported signer metadata is unavailable.")?;
    if matches.next().is_some() {
        return Err("Duplicate wallet records prevent safe pairing.".into());
    }
    Ok(wallet)
}

fn source_binding(source: &CubeSettings) -> Result<ImportedClaimPair, String> {
    if source.network != ChainId::Bitcoin || source.vault_wallet_id.is_some() {
        return Err("Open an imported Bitcoin wallet to create its Bitcoin Blake2b copy.".into());
    }
    let import = source
        .bitcoin_import
        .as_ref()
        .ok_or("The source imported wallet is missing.")?;
    let wallet = import
        .active_attachment()
        .ok_or("The source imported wallet is not installed.")?;
    wallet.validate(source.network)?;
    Ok(ImportedClaimPair {
        source_cube_id: source.id.clone(),
        source_operation_id: import.operation_id,
        source_wallet: wallet.clone(),
    })
}

fn paired_wallet(binding: &ImportedClaimPair) -> Result<ImportedWalletAttachment, String> {
    ImportedWalletAttachment::new(
        binding
            .source_wallet
            .descriptor
            .paired_blake2b()
            .map_err(|e| e.to_string())?,
        ChainId::BitcoinBlake2b,
        binding.source_wallet.birthday,
        binding.source_wallet.initial_lookahead,
    )
}

fn validate_target(source: &CubeSettings, target: &CubeSettings) -> Result<(), String> {
    let binding = source_binding(source)?;
    let wallet = paired_wallet(&binding)?;
    if target.id != target_cube_id(&source.id)
        || target.network != ChainId::BitcoinBlake2b
        || target.imported_claim_source.as_deref() != Some(&binding)
        || target.vault_wallet_id.is_some()
        || target.master_signer_fingerprint != source.master_signer_fingerprint
        || target.master_signer_fingerprint.is_none()
        // The copy unlocks the way its source does: the same PIN, or the
        // same passkey.
        || target.passkey_metadata != source.passkey_metadata
        || target.bitcoin_import.as_ref().and_then(|i| i.attachment()) != Some(&wallet)
    {
        return Err(
            "The Bitcoin Blake2b copy conflicts with this wallet. No wallet was replaced.".into(),
        );
    }
    Ok(())
}

/// Validate a restored target without requiring its source to be present.
/// Spending/Claim handoff still requires validate_pair and both durable records.
pub fn validate_target_record(target: &CubeSettings) -> Result<(), String> {
    let binding = target
        .imported_claim_source
        .as_ref()
        .ok_or("Paired source metadata is unavailable.")?;
    binding.source_wallet.validate(ChainId::Bitcoin)?;
    let mut source = CubeSettings::new_with_raw_id(
        binding.source_cube_id.clone(),
        String::new(),
        ChainId::Bitcoin,
    );
    source.master_signer_fingerprint = target.master_signer_fingerprint;
    source.passkey_metadata = target.passkey_metadata.clone();
    source.bitcoin_import = Some(Box::new(BitcoinImport {
        operation_id: binding.source_operation_id,
        stage: BitcoinImportStage::Ready {
            wallet: binding.source_wallet.clone(),
        },
        last_error: None,
    }));
    validate_target(&source, target)
}

fn matching_signers(source: &WalletSettings, target: &WalletSettings) -> bool {
    serde_json::to_value((
        &source.keys,
        &source.hardware_wallets,
        source.keychain_keys_recorded,
    ))
    .ok()
        == serde_json::to_value((
            &target.keys,
            &target.hardware_wallets,
            target.keychain_keys_recorded,
        ))
        .ok()
}

/// Recheck both durable records on every Claim handoff. No source mutation.
pub fn validate_pair(
    root: &CoincubeDirectory,
    target: &CubeSettings,
) -> Result<CubeSettings, String> {
    let binding = target
        .imported_claim_source
        .as_ref()
        .ok_or("This Cube is not a paired imported Claim destination.")?;
    let source_settings = Settings::from_file(&root.network_directory(ChainId::Bitcoin))
        .map_err(|e| e.to_string())?;
    let source = unique_cube(&source_settings, &binding.source_cube_id)?;
    validate_target(source, target)?;
    let target_settings =
        Settings::from_file(&root.network_directory(target.network)).map_err(|e| e.to_string())?;
    let saved_target = unique_cube(&target_settings, &target.id)?;
    validate_target(source, saved_target)?;
    let source_wallet = unique_wallet(&source_settings, &binding.source_wallet.wallet_id)?;
    let wallet = paired_wallet(binding)?;
    let target_wallet = unique_wallet(&target_settings, &wallet.wallet_id)?;
    if !matching_signers(source_wallet, target_wallet) {
        return Err("The paired imported wallet signer metadata changed.".into());
    }
    Ok(source.clone())
}

/// Published initialization metadata is sufficient for a still-syncing pair:
/// do not reopen a database that its daemon may already have running.
fn validate_syncing_discovery(
    directory: &NetworkDirectory,
    target: &CubeSettings,
) -> Result<(), String> {
    let import = target
        .bitcoin_import
        .as_ref()
        .ok_or("The paired import is unavailable.")?;
    let missing = "The unfinished paired wallet has no verified copied discovery range. Finish or resolve its pending import before retrying the copy.";
    let journal = read_journal(&journal_path(directory, import.operation_id))?.ok_or(missing)?;
    if journal.cube_id != target.id
        || journal.operation_id != import.operation_id
        || Some(&journal.wallet) != import.active_attachment()
        || journal.restored_discovery_target.is_none()
        || !matches!(journal.phase, Phase::Published | Phase::Attached)
        || !owns_directory(&journal.final_path(directory), &journal)?
    {
        return Err(missing.into());
    }
    Ok(())
}

/// Create, or reuse, this imported Bitcoin wallet's Bitcoin Blake2b copy:
/// a Cube of its own holding the paired imported wallet, unlocked with the
/// same seed, and the same PIN or passkey. `seed_password` is what the
/// source's seed file is encrypted under: its PIN, or for a passkey Cube the
/// password derived from its seed ([`crate::app::session::seed_file_password`]).
/// Uses the normal staged import journal, and reuses a finished pair without
/// reopening its database.
pub async fn install_pair(
    root: &CoincubeDirectory,
    source: &CubeSettings,
    signer: &Signer,
    seed_password: &str,
    backend: coincubed::config::BitcoinBackend,
) -> Result<CubeSettings, String> {
    let binding = source_binding(source)?;
    if source.master_signer_fingerprint != Some(signer.fingerprint()) || seed_password.is_empty() {
        return Err("Unlock this Cube again before creating the copy.".into());
    }
    let source_dir = root.network_directory(source.network);
    let source_settings = Settings::from_file(&source_dir).map_err(|e| e.to_string())?;
    let current = unique_cube(&source_settings, &source.id)?;
    if source_binding(current)? != binding
        || current.master_signer_fingerprint != source.master_signer_fingerprint
        || current.backed_up != source.backed_up
        || current.passkey_metadata != source.passkey_metadata
    {
        return Err("This Cube changed. Open Split coins and try again.".into());
    }
    // A completed pair is reused without reopening or rewriting its database.
    // Source discovery may grow while the fork daemon is running.
    let target_dir = root.network_directory(ChainId::BitcoinBlake2b);
    if target_dir
        .path()
        .join(settings::SETTINGS_FILE_NAME)
        .exists()
    {
        let saved = Settings::from_file(&target_dir).map_err(|e| e.to_string())?;
        if saved
            .cubes
            .iter()
            .any(|c| c.id == target_cube_id(&source.id))
        {
            let prior = unique_cube(&saved, &target_cube_id(&source.id))?;
            validate_target(source, prior)?;
            if prior.has_imported_bitcoin() {
                validate_pair(root, prior)?;
                if !prior
                    .bitcoin_import
                    .as_ref()
                    .is_some_and(|import| import.is_ready())
                {
                    validate_syncing_discovery(&target_dir, prior)?;
                }
                return Ok(prior.clone());
            }
            if read_journal(&journal_path(
                &target_dir,
                prior
                    .bitcoin_import
                    .as_ref()
                    .ok_or("The paired import is unavailable.")?
                    .operation_id,
            ))?
            .is_some_and(|journal| journal.restored_discovery_target.is_none())
            {
                return Err("This unfinished paired wallet has no saved discovery range. Resolve its pending import before retrying the copy; no wallet data was changed.".into());
            }
        }
    }
    let source_data = source_dir.coincubed_data_directory(&binding.source_wallet.wallet_id);
    let source_config =
        Config::from_file(Some(source_data.path().join(CONFIG_FILE))).map_err(|e| e.to_string())?;
    if source_config.bitcoin_config.chain != source.network
        || source_config.main_descriptor.as_imported() != Some(&binding.source_wallet.descriptor)
        || source_config.import_birthday != Some(binding.source_wallet.birthday)
        || source_config.import_initial_lookahead != Some(binding.source_wallet.initial_lookahead)
    {
        return Err("This wallet's storage changed. Open Split coins and try again.".into());
    }
    let address_state = coincubed::read_imported_claim_state(&source_config, &source_data)
        .map_err(|e| e.to_string())?;
    if !address_state.history_complete {
        return Err("Wait for this wallet's scan to finish before creating the copy.".into());
    }
    let discovery = address_state
        .discovery_target
        .max(address_state.receive_scanned)
        .max(address_state.change_scanned)
        .max(
            address_state
                .receive_index
                .checked_add(1)
                .ok_or("Source address range overflow")?,
        )
        .max(
            address_state
                .change_index
                .checked_add(1)
                .ok_or("Source address range overflow")?,
        );
    if discovery > 10_000 {
        return Err("This wallet's address range is too large to copy.".into());
    }
    let source_metadata =
        unique_wallet(&source_settings, &binding.source_wallet.wallet_id)?.clone();
    let wallet = paired_wallet(&binding)?;
    let directory = root.network_directory(ChainId::BitcoinBlake2b);
    directory.init().map_err(|e| e.to_string())?;
    check_duress(&directory)?;
    let mut target = CubeSettings::new_with_raw_id(
        target_cube_id(&source.id),
        format!("{} · BTCB2", source.name),
        ChainId::BitcoinBlake2b,
    );
    target.master_signer_fingerprint = source.master_signer_fingerprint;
    target.backed_up = source.backed_up;
    target.mfa_done = source.mfa_done;
    target.passkey_metadata = source.passkey_metadata.clone();
    target.imported_claim_source = Some(Box::new(binding));
    target.bitcoin_import = Some(Box::new(BitcoinImport {
        operation_id: Uuid::parse_str(&target.id).map_err(|e| e.to_string())?,
        stage: BitcoinImportStage::Installing {
            wallet: wallet.clone(),
        },
        last_error: None,
    }));
    let prior = read_journal(&journal_path(
        &directory,
        target.bitcoin_import.as_ref().unwrap().operation_id,
    ))?;
    if prior
        .as_ref()
        .is_some_and(|journal| journal.restored_discovery_target.is_none())
    {
        return Err("This unfinished paired wallet has no saved discovery range. Resolve its pending import before retrying the copy; no wallet data was changed.".into());
    }
    settings::update_settings_file_checked(&directory, |mut saved| {
        if saved.cubes.iter().any(|c|c.id == target.id) { validate_target(source,unique_cube(&saved,&target.id).map_err(SettingsError::WritingFile)?).map_err(SettingsError::WritingFile)?; }
        else {
            if saved.cubes.iter().any(|c|c.bitcoin_import.as_ref().and_then(|i|i.attachment()).is_some_and(|w|w.wallet_id == wallet.wallet_id)) { return Err(SettingsError::WritingFile("This imported fork wallet already belongs to another Cube. Resolve that attachment first.".into())); }
            saved.cubes.push(target.clone());
        }
        let mut metadata = source_metadata.clone();
        metadata.descriptor_checksum = wallet.wallet_id.descriptor_checksum.clone();
        metadata.pinned_at = wallet.wallet_id.timestamp;
        if let Some(prior) = saved.wallets.iter().find(|w|w.wallet_id() == wallet.wallet_id) {
            unique_wallet(&saved,&wallet.wallet_id).map_err(SettingsError::WritingFile)?;
            if !matching_signers(&metadata,prior) { return Err(SettingsError::WritingFile("Paired signer metadata conflicts; nothing was replaced.".into())); }
        } else { saved.wallets.push(metadata); }
        Ok(Some(saved))
    }).await.map_err(|e|e.to_string())?;
    let secret = crate::services::unlock::device_secret::get_or_create(root.path(), &target.id)
        .map_err(|e| e.to_string())?;
    crate::installer::persist_cube_master_seed(
        signer,
        root,
        ChainId::BitcoinBlake2b,
        seed_password,
        &target.id,
        Some(&secret),
    )
    .map_err(|e| e.to_string())?;
    crate::installer::ensure_gui_config(&directory, false).map_err(|e| e.to_string())?;
    let mut config = Config::new(
        coincubed::config::BitcoinConfig::new(target.network, std::time::Duration::from_secs(10)),
        Some(backend),
        log::LevelFilter::Info,
        WalletDescriptor::Imported(wallet.descriptor.clone()),
        directory.coincubed_data_directory(&wallet.wallet_id),
    );
    config.import_birthday = Some(wallet.birthday);
    config.import_initial_lookahead = Some(wallet.initial_lookahead);
    let restore_state = prior
        .as_ref()
        .and_then(|journal| journal.restored_address_state)
        .unwrap_or(RestoredAddressState {
            receive_index: address_state.receive_index,
            change_index: address_state.change_index,
        });
    let discovery = prior
        .and_then(|journal| journal.restored_discovery_target)
        .unwrap_or(discovery);
    install_restored_with_discovery(
        &directory,
        &target.id,
        target.bitcoin_import.as_ref().unwrap().operation_id,
        wallet.clone(),
        config,
        restore_state,
        discovery,
    )
    .await?;
    target = Settings::from_file(&directory)
        .map_err(|e| e.to_string())?
        .cubes
        .into_iter()
        .find(|c| c.id == target.id)
        .ok_or("Paired Cube disappeared.")?;
    validate_pair(root, &target)?;
    Ok(target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A ready imported Bitcoin wallet Cube (single-sig wpkh, birthday 0,
    /// lookahead 200).
    pub(crate) fn imported_cube() -> CubeSettings {
        let descriptor = coincube_core::wallet_descriptor::ImportedWalletDescriptor::parse(
            "wpkh([6cb116eb/84h/0h/0h]xpub6C6c6ShFaUMjohRwYGnrDr5kbrnUjBeGh5U8BPAjtZnZ2D7WpRidu2SUNpBVfmMaw8TpDA7quYNW4iNwGYaq9iAwp5fzHLCXJwTqe76WpmF/<0;1>/*)",
            None, ChainId::Bitcoin,
        ).unwrap();
        let wallet = crate::app::settings::ImportedWalletAttachment::new(
            descriptor,
            ChainId::Bitcoin,
            0,
            200,
        )
        .unwrap();
        let mut cube = CubeSettings::new("Imported wallet".into(), ChainId::Bitcoin);
        cube.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: crate::app::settings::BitcoinImportStage::Ready { wallet },
            last_error: None,
        }));
        cube
    }

    pub(crate) fn pair() -> (CubeSettings, CubeSettings) {
        let mut source = imported_cube();
        source.master_signer_fingerprint = Some([1, 2, 3, 4].into());
        source.backed_up = true;
        let binding = source_binding(&source).unwrap();
        let mut target = CubeSettings::new_with_raw_id(
            target_cube_id(&source.id),
            "Paired".into(),
            ChainId::BitcoinBlake2b,
        );
        target.master_signer_fingerprint = source.master_signer_fingerprint;
        target.backed_up = source.backed_up;
        target.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: Uuid::new_v4(),
            stage: BitcoinImportStage::Ready {
                wallet: paired_wallet(&binding).unwrap(),
            },
            last_error: None,
        }));
        target.imported_claim_source = Some(Box::new(binding));
        (source, target)
    }

    #[test]
    fn exact_pair_keeps_branches_and_external_origins_but_separates_identity() {
        let (source, target) = pair();
        validate_target(&source, &target).unwrap();
        let a = source
            .bitcoin_import
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        let b = target
            .bitcoin_import
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        assert_eq!(
            a.descriptor.receive_descriptor(),
            b.descriptor.receive_descriptor()
        );
        assert_eq!(
            a.descriptor.change_descriptor(),
            b.descriptor.change_descriptor()
        );
        assert_ne!(a.wallet_id, b.wallet_id);
        assert_ne!(target.id, source.id);
        assert_eq!(target.id, target_cube_id(&source.id));
        assert!(target.spark_stable_balance.is_none());
    }
    #[test]
    fn pairing_rejects_changed_source_operation_target_or_unlock_identity() {
        let (source, target) = pair();
        let mut wrong = source.clone();
        wrong.bitcoin_import.as_mut().unwrap().operation_id = Uuid::new_v4();
        assert!(validate_target(&wrong, &target).is_err());
        let mut wrong = target.clone();
        wrong.id = Uuid::new_v4().to_string();
        assert!(validate_target(&source, &wrong).is_err());
        let mut wrong = target.clone();
        wrong.master_signer_fingerprint = Some([9, 9, 9, 9].into());
        assert!(validate_target(&source, &wrong).is_err());
        let mut wrong = target;
        wrong
            .imported_claim_source
            .as_mut()
            .unwrap()
            .source_wallet
            .birthday += 1;
        assert!(validate_target(&source, &wrong).is_err());
    }
    #[test]
    fn duplicates_are_refused_instead_of_selecting_first() {
        let (source, _) = pair();
        let saved = Settings {
            cubes: vec![source.clone(), source.clone()],
            ..Default::default()
        };
        assert!(unique_cube(&saved, &source.id).is_err());
    }

    #[tokio::test]
    async fn ready_pair_reuses_without_opening_running_database_or_reinstalling_snapshot() {
        let (mut source, mut target) = pair();
        let signer =
            Signer::generate(coincube_core::miniscript::bitcoin::Network::Bitcoin).unwrap();
        source.master_signer_fingerprint = Some(signer.fingerprint());
        target.master_signer_fingerprint = source.master_signer_fingerprint;
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("pair-reuse-{}", Uuid::new_v4())),
        );
        let source_wallet = source
            .bitcoin_import
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        let target_wallet = target
            .bitcoin_import
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        let mut metadata = WalletSettings {
            name: "Synthetic external signer".into(),
            alias: None,
            descriptor_checksum: source_wallet.wallet_id.descriptor_checksum.clone(),
            pinned_at: None,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let source_dir = root.network_directory(ChainId::Bitcoin);
        source_dir.init().unwrap();
        settings::update_settings_file(&source_dir, |mut s| {
            s.cubes.push(source.clone());
            s.wallets.push(metadata.clone());
            Some(s)
        })
        .await
        .unwrap();
        metadata.descriptor_checksum = target_wallet.wallet_id.descriptor_checksum.clone();
        let target_dir = root.network_directory(ChainId::BitcoinBlake2b);
        target_dir.init().unwrap();
        settings::update_settings_file(&target_dir, |mut s| {
            s.cubes.push(target.clone());
            s.wallets.push(metadata);
            Some(s)
        })
        .await
        .unwrap();
        // A running target may have WAL sidecars; a growing source may also
        // be unavailable for preflight. Reuse needs neither database.
        let data = target_dir.coincubed_data_directory(&target_wallet.wallet_id);
        std::fs::create_dir_all(data.path()).unwrap();
        std::fs::write(
            data.path().join("coincubed.sqlite3-wal"),
            b"synthetic-running-database",
        )
        .unwrap();
        let backend =
            coincubed::config::BitcoinBackend::Electrum(coincubed::config::ElectrumConfig {
                addr: "tcp://127.0.0.1:50001".into(),
                validate_domain: true,
            });
        for _ in 0..2 {
            let reused = install_pair(&root, &source, &signer, "1234", backend.clone())
                .await
                .unwrap();
            assert_eq!(reused.id, target.id);
            assert!(reused.bitcoin_import.unwrap().is_ready());
        }
        assert_eq!(
            std::fs::read(data.path().join("coincubed.sqlite3-wal")).unwrap(),
            b"synthetic-running-database"
        );
        std::fs::remove_dir_all(root.path()).unwrap();
    }
    #[tokio::test]
    async fn a_copy_needs_this_cubes_own_signer_and_pin() {
        let (mut source, _) = pair();
        let signer =
            Signer::generate(coincube_core::miniscript::bitcoin::Network::Bitcoin).unwrap();
        let other = Signer::generate(coincube_core::miniscript::bitcoin::Network::Bitcoin).unwrap();
        source.master_signer_fingerprint = Some(signer.fingerprint());
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("pair-unlock-{}", Uuid::new_v4())),
        );
        let backend =
            coincubed::config::BitcoinBackend::Electrum(coincubed::config::ElectrumConfig {
                addr: "tcp://127.0.0.1:50001".into(),
                validate_domain: true,
            });
        for (signer, pin) in [(&other, "1234"), (&signer, "")] {
            let error = install_pair(&root, &source, signer, pin, backend.clone())
                .await
                .unwrap_err();
            assert!(error.contains("Unlock this Cube"), "{}", error);
        }
        // A passkey Cube or a Cube holding a Vault is not an imported source.
        let mut vault = source.clone();
        vault.vault_wallet_id = Some(WalletId::new("abcd".into(), Some(1)));
        assert!(install_pair(&root, &vault, &signer, "1234", backend)
            .await
            .is_err());
        assert!(!root.path().exists(), "nothing was written");
    }

    /// Every imported shape pairs onto Bitcoin Blake2b with the same branches
    /// and a wallet identity of its own.
    #[test]
    fn every_imported_shape_pairs_onto_bitcoin_blake2b() {
        use crate::services::split_test_wallets::{imported_vault, SHAPES};
        for shape in SHAPES {
            let descriptor = imported_vault(shape)
                .main_descriptor
                .as_imported()
                .unwrap()
                .clone();
            let mut source = imported_cube();
            source.master_signer_fingerprint = Some([1, 2, 3, 4].into());
            source.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Ready {
                wallet: ImportedWalletAttachment::new(descriptor.clone(), ChainId::Bitcoin, 0, 20)
                    .unwrap(),
            };
            let binding = source_binding(&source).unwrap();
            let paired = paired_wallet(&binding).unwrap();
            assert_eq!(
                paired.descriptor.chain(),
                ChainId::BitcoinBlake2b,
                "{:?}",
                shape
            );
            assert_eq!(
                paired.descriptor.receive_descriptor(),
                descriptor.receive_descriptor(),
                "{:?}",
                shape
            );
            assert_eq!(
                paired.descriptor.change_descriptor(),
                descriptor.change_descriptor(),
                "{:?}",
                shape
            );
            assert_ne!(
                paired.wallet_id, binding.source_wallet.wallet_id,
                "{:?}",
                shape
            );
        }
    }

    /// Every message the copy can show has a Spanish translation.
    #[test]
    fn the_copy_messages_are_translated() {
        let messages = [
            "Sign in to COINCUBE | Connect to create the Bitcoin Blake2b copy.",
            "Unlock this Cube with its PIN again to create the copy.",
            "Bitcoin Blake2b copy created. Open it from the Cube switcher.",
            "Unlock this Cube again before creating the copy.",
            "Unlock this Cube with its passkey again to create the copy.",
            "This Cube changed. Open Split coins and try again.",
            "This unfinished paired wallet has no saved discovery range. Resolve its pending import before retrying the copy; no wallet data was changed.",
            "The unfinished paired wallet has no verified copied discovery range. Finish or resolve its pending import before retrying the copy.",
            "This wallet's storage changed. Open Split coins and try again.",
            "Wait for this wallet's scan to finish before creating the copy.",
            "This wallet's address range is too large to copy.",
            "Open an imported Bitcoin wallet to create its Bitcoin Blake2b copy.",
            "The Bitcoin Blake2b copy conflicts with this wallet. No wallet was replaced.",
            "The paired imported wallet signer metadata changed.",
            "The source imported wallet is missing.",
            "The source imported wallet is not installed.",
            "The paired Cube is no longer available.",
            "Imported signer metadata is unavailable.",
            "Duplicate Cube records prevent safe pairing.",
            "Duplicate wallet records prevent safe pairing.",
        ];
        let source = include_str!("pairing.rs");
        let app = include_str!("../../app/mod.rs");
        for message in messages {
            assert!(
                source.contains(message) || app.contains(message),
                "not used any more: {}",
                message
            );
            let spanish =
                coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
                    coincube_ui::locale::copy(message).to_string()
                });
            assert_ne!(spanish, message, "no Spanish for: {}", message);
        }
    }

    /// A passkey Cube pairs too: its copy carries the same passkey, and a
    /// copy that doesn't is refused, in either direction.
    #[test]
    fn a_passkey_cubes_copy_unlocks_with_the_same_passkey() {
        let passkey = crate::app::settings::PasskeyMetadata {
            credential_id: "synthetic-credential".to_string(),
            rp_id: "coincube.io".to_string(),
            created_at: 0,
            label: None,
        };
        let (mut source, mut target) = pair();
        source.passkey_metadata = Some(passkey.clone());
        assert!(source.is_passkey_cube());
        assert!(
            validate_target(&source, &target).is_err(),
            "a copy without the passkey"
        );
        target.passkey_metadata = Some(passkey.clone());
        validate_target(&source, &target).unwrap();
        validate_target_record(&target).unwrap();
        let mut other = passkey;
        other.credential_id = "another-credential".to_string();
        target.passkey_metadata = Some(other);
        assert!(
            validate_target(&source, &target).is_err(),
            "another passkey"
        );
        // A PIN source's copy must not claim a passkey.
        let (source, mut target) = pair();
        target.passkey_metadata = source.passkey_metadata.clone();
        validate_target(&source, &target).unwrap();
        target.passkey_metadata = Some(crate::app::settings::PasskeyMetadata {
            credential_id: "synthetic-credential".to_string(),
            rp_id: "coincube.io".to_string(),
            created_at: 0,
            label: None,
        });
        assert!(validate_target(&source, &target).is_err());
    }
}
