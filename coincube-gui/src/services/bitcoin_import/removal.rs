//! Remove one imported wallet only when its durable owner is the selected Cube.
//!
//! This is intentionally separate from `delete_wallet`: imported wallets have
//! no Vault id, and a wallet-id-derived path alone is not proof of ownership.

use std::{fs, io::ErrorKind, path::Path};

use async_fd_lock::LockWrite;

use super::{
    check_directory_chain, journal_path, owns_directory, read_journal, sync_directory, Journal,
    OWNER_FILE,
};
use crate::{
    app::settings::{self, CubeSettings, ImportedWalletAttachment, Settings, SettingsError},
    dir::NetworkDirectory,
};

fn checked_live_cube(settings: &Settings, expected: &CubeSettings) -> Result<bool, String> {
    let Some(expected_import) = expected.bitcoin_import.as_ref() else {
        return Err("The selected Cube has no Bitcoin import.".into());
    };
    let Some(cube) = settings.cubes.iter().find(|cube| cube.id == expected.id) else {
        return Ok(false); // A completed removal may be retried after a crash.
    };
    if cube.network != expected.network
        || cube.vault_wallet_id.is_some()
        || cube.bitcoin_import.as_ref().is_none_or(|current| {
            current.operation_id != expected_import.operation_id
                || current.attachment() != expected_import.attachment()
        })
    {
        return Err("This Cube's Bitcoin import changed. No wallet files were removed.".into());
    }
    Ok(true)
}

fn checked_owner_directory(path: &Path, journal: &Journal) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if !metadata.file_type().is_dir() => {
            Err("An import storage path is not a real directory; no files were removed.".into())
        }
        Ok(_) if owns_directory(path, journal)? => Ok(true),
        Ok(_) => {
            Err("The import storage belongs to another operation; no files were removed.".into())
        }
    }
}

fn owner_at(path: &Path) -> Result<Option<Journal>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if !metadata.file_type().is_dir() => {
            Err("An import storage path is not a real directory; no files were removed.".into())
        }
        Ok(_) => read_journal(&path.join(OWNER_FILE)),
    }
}

fn require_real_parent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => {
            Err("An import storage parent is not a real directory; no files were removed.".into())
        }
    }
}

fn check_journal(
    journal: &Journal,
    expected: &CubeSettings,
    attachment: Option<&ImportedWalletAttachment>,
) -> Result<(), String> {
    let operation_id = expected
        .bitcoin_import
        .as_ref()
        .expect("checked import")
        .operation_id;
    journal.wallet.validate(expected.network)?;
    if journal.cube_id != expected.id
        || journal.operation_id != operation_id
        || attachment.is_some_and(|wallet| wallet != &journal.wallet)
    {
        return Err("The import ownership record does not match this Cube.".into());
    }
    Ok(())
}

fn remove_owned_directory(path: &Path, parent: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => sync_directory(parent),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether anything (file, directory or dangling link) exists at `path`.
fn storage_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Delete the selected Cube's imported wallet and its exact operation record.
/// All present storage is checked against the journal and owner marker before
/// mutation. Settings are changed last so an interrupted cleanup remains
/// visible and can be retried from the Cube's delete dialog.
pub async fn remove_cube_import(
    directory: &NetworkDirectory,
    expected: &CubeSettings,
) -> Result<(), String> {
    remove_cube_import_within(directory, expected, crate::daemon::stopping::WAIT_LIMIT).await
}

async fn remove_cube_import_within(
    directory: &NetworkDirectory,
    expected: &CubeSettings,
    stop_limit: std::time::Duration,
) -> Result<(), String> {
    let intent = expected
        .bitcoin_import
        .as_ref()
        .ok_or("The selected Cube has no Bitcoin import.")?;
    check_directory_chain(directory, expected.network)?;
    let _lock = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.path().join("wallet-import.lock"))
        .await
        .map_err(|error| error.to_string())?
        .lock_write()
        .await
        .map_err(|error| format!("Cannot lock wallet import: {error:?}"))?;

    let settings = match Settings::from_file(directory) {
        Ok(settings) => settings,
        Err(SettingsError::NotFound) => Settings::default(),
        Err(error) => return Err(error.to_string()),
    };
    let cube_present = checked_live_cube(&settings, expected)?;
    let expected_attachment = intent.attachment();
    if let Some(wallet) = expected_attachment {
        wallet.validate(expected.network)?;
    }

    let journal_file = journal_path(directory, intent.operation_id);
    let stage = directory
        .path()
        .join("data")
        .join(format!(".import-{}", intent.operation_id));
    require_real_parent(&directory.path().join("data"))?;
    require_real_parent(&directory.path().join("imports"))?;
    let file_journal = read_journal(&journal_file)?;
    let staged_owner = owner_at(&stage)?;
    // An attached wallet's identity gives us the exact final path even if a
    // crash removed its journal. A pending operation without a journal has no
    // published destination by construction.
    let final_from_attachment = expected_attachment.map(|wallet| {
        directory
            .coincubed_data_directory(&wallet.wallet_id)
            .path()
            .to_path_buf()
    });
    let final_owner = match final_from_attachment.as_ref() {
        Some(path) => owner_at(path)?,
        None => None,
    };
    let journal = file_journal
        .as_ref()
        .or(staged_owner.as_ref())
        .or(final_owner.as_ref());
    if journal.is_none() {
        if let Some(path) = final_from_attachment.clone() {
            if super::blocking_fs(move || Ok(storage_present(&path))).await? {
                return Err(
                    "Imported wallet storage lacks an owner; no files were removed.".into(),
                );
            }
        }
    }
    for owner in [
        file_journal.as_ref(),
        staged_owner.as_ref(),
        final_owner.as_ref(),
    ]
    .iter()
    .flatten()
    {
        check_journal(owner, expected, expected_attachment)?;
        if let Some(primary) = journal {
            if owner.cube_id != primary.cube_id
                || owner.operation_id != primary.operation_id
                || owner.wallet != primary.wallet
            {
                return Err("The import ownership records disagree; no files were removed.".into());
            }
        }
    }
    let destination = journal.map(|owner| owner.final_path(directory));
    if let (Some(expected_path), Some(actual_path)) =
        (final_from_attachment.as_ref(), destination.as_ref())
    {
        if expected_path != actual_path {
            return Err("The import destination differs from this Cube's wallet.".into());
        }
    }
    let stage_present = match journal {
        Some(owner) => checked_owner_directory(&stage, owner)?,
        None => {
            if stage.exists() {
                return Err("Staged import data lacks an owner; no files were removed.".into());
            }
            false
        }
    };
    let final_present = match (journal, destination.as_ref()) {
        (Some(owner), Some(path)) => checked_owner_directory(path, owner)?,
        (None, Some(path)) if path.exists() => {
            return Err("Imported wallet storage lacks an owner; no files were removed.".into());
        }
        _ => false,
    };
    // A stale modal after successful deletion carries a wallet id but no
    // longer proves ownership of a settings row. There is nothing left to
    // retry when both the Cube and its exact operation records are absent.
    if !cube_present && journal.is_none() && !stage_present && !final_present {
        return Ok(());
    }
    let wallet_id = journal
        .map(|owner| &owner.wallet.wallet_id)
        .or_else(|| expected_attachment.map(|wallet| &wallet.wallet_id));
    if let Some(wallet_id) = wallet_id {
        if settings.cubes.iter().any(|other| {
            other.id != expected.id
                && (other.vault_wallet_id.as_ref() == Some(wallet_id)
                    || other
                        .bitcoin_import
                        .as_ref()
                        .and_then(|import| import.attachment())
                        .is_some_and(|wallet| &wallet.wallet_id == wallet_id))
        }) {
            return Err("Another Cube still references this wallet; no files were removed.".into());
        }
    }
    // Deleting a Cube right after locking it: the imported wallet's daemon may
    // still be stopping in its directory (see `crate::daemon::stopping`).
    // If it outlasts the wait, remove nothing.
    for dir in [Some(stage.clone()), destination.clone()].iter().flatten() {
        if !crate::daemon::stopping::wait_for_async_within(dir.clone(), stop_limit).await {
            return Err(crate::daemon::stopping::STILL_STOPPING.to_string());
        }
    }
    if let Some(attachment) = expected_attachment {
        if expected.network.is_blake2b() {
            // The local-key repository refuses every BTCB2 scope, so a BTCB2
            // import can never have admitted keys. Storage under its
            // operation id is unexplained: keep it and refuse the delete.
            let key_root = directory
                .path()
                .join("imported-local-keys")
                .join(intent.operation_id.to_string());
            if super::blocking_fs(move || Ok(storage_present(&key_root))).await? {
                return Err(
                    "Unexpected imported key storage for a Bitcoin Blake2b Cube; no files were removed."
                        .into(),
                );
            }
        } else {
            crate::services::imported_keys::LocalKeyRepository::new(
                directory, expected, attachment,
            )?
            .remove_operation_storage()?;
        }
    }

    // Keep the Cube visible until cleanup finishes. An interrupted removal
    // can be retried from its delete dialog, even if some owned files are gone.
    if stage_present {
        remove_owned_directory(
            &stage,
            stage.parent().ok_or("Missing import data directory")?,
        )?;
    }
    if final_present {
        let path = destination.as_ref().expect("checked destination");
        remove_owned_directory(path, path.parent().ok_or("Missing import data directory")?)?;
    }
    match fs::remove_file(&journal_file) {
        Ok(()) => sync_directory(
            journal_file
                .parent()
                .ok_or("Missing import journal directory")?,
        )?,
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => return Err(error.to_string()),
    }
    settings::update_settings_file_checked(directory, |mut current| {
        checked_live_cube(&current, expected).map_err(SettingsError::WritingFile)?;
        if let Some(wallet_id) = wallet_id {
            if current.cubes.iter().any(|other| {
                other.id != expected.id
                    && (other.vault_wallet_id.as_ref() == Some(wallet_id)
                        || other
                            .bitcoin_import
                            .as_ref()
                            .and_then(|import| import.attachment())
                            .is_some_and(|wallet| &wallet.wallet_id == wallet_id))
            }) {
                return Err(SettingsError::WritingFile(
                    "Another Cube now references this wallet; retry its deletion.".into(),
                ));
            }
            current
                .wallets
                .retain(|wallet| wallet.wallet_id() != *wallet_id);
        }
        current.cubes.retain(|cube| cube.id != expected.id);
        if current.cubes.is_empty() && current.wallets.is_empty() {
            Ok(None)
        } else {
            Ok(Some(current))
        }
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr, time::Duration};

    use coincube_core::{
        miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
            Network,
        },
        wallet_descriptor::{ImportedWalletDescriptor, WalletDescriptor},
    };
    use coincubed::config::Config;
    use uuid::Uuid;

    use super::*;
    use crate::{
        app::settings::{BitcoinImport, BitcoinImportStage},
        chain::ChainId,
    };

    struct Fixture {
        root: PathBuf,
        directory: NetworkDirectory,
        cube: CubeSettings,
        other: CubeSettings,
        wallet: ImportedWalletAttachment,
        config: Config,
    }

    impl Fixture {
        async fn new() -> Self {
            Self::on_chain(ChainId::Regtest).await
        }

        async fn on_chain(chain: ChainId) -> Self {
            let root = std::env::temp_dir().join(format!("import-removal-test-{}", Uuid::new_v4()));
            let directory = NetworkDirectory::new(root.join(chain.dir_name()));
            directory.init().unwrap();
            let secp = Secp256k1::new();
            let network = chain.bitcoin_network();
            let coin = if network == Network::Bitcoin { 0 } else { 1 };
            let master = Xpriv::new_master(network, &[51; 32]).unwrap();
            let account = master
                .derive_priv(
                    &secp,
                    &DerivationPath::from_str(&format!("m/84'/{coin}'/0'")).unwrap(),
                )
                .unwrap();
            let key = format!(
                "[{}/84'/{coin}'/0']{}",
                master.fingerprint(&secp),
                Xpub::from_priv(&secp, &account)
            );
            let descriptor = ImportedWalletDescriptor::parse(
                &format!("wpkh({key}/0/*)"),
                Some(&format!("wpkh({key}/1/*)")),
                chain,
            )
            .unwrap();
            let wallet = ImportedWalletAttachment::new(descriptor.clone(), chain, 0, 20).unwrap();
            let config = Config::new(
                coincubed::config::BitcoinConfig::new(chain, Duration::from_secs(30)),
                None,
                log::LevelFilter::Info,
                WalletDescriptor::Imported(descriptor),
                directory.coincubed_data_directory(&wallet.wallet_id),
            );
            let mut cube = CubeSettings::new("Selected".into(), chain);
            cube.bitcoin_import = Some(Box::new(BitcoinImport::pending()));
            let other = CubeSettings::new("Unrelated".into(), chain);
            settings::update_settings_file(&directory, |mut settings| {
                settings.cubes = vec![other.clone(), cube.clone()];
                Some(settings)
            })
            .await
            .unwrap();
            Self {
                root,
                directory,
                cube,
                other,
                wallet,
                config,
            }
        }

        fn operation_id(&self) -> Uuid {
            self.cube.bitcoin_import.as_ref().unwrap().operation_id
        }

        async fn install(&mut self) {
            super::super::install(
                &self.directory,
                &self.cube.id,
                self.operation_id(),
                self.wallet.clone(),
                self.config.clone(),
            )
            .await
            .unwrap();
            self.cube = Settings::from_file(&self.directory)
                .unwrap()
                .cubes
                .into_iter()
                .find(|cube| cube.id == self.cube.id)
                .unwrap();
            assert!(matches!(
                self.cube.bitcoin_import.as_ref().unwrap().stage,
                BitcoinImportStage::Syncing { .. }
            ));
        }

        fn destination(&self) -> PathBuf {
            self.directory
                .coincubed_data_directory(&self.wallet.wallet_id)
                .path()
                .to_path_buf()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn pending_without_journal_removes_only_selected_cube() {
        let f = Fixture::new().await;
        let unrelated = f.directory.path().join("keep-me");
        fs::write(&unrelated, b"unrelated").unwrap();
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        let saved = Settings::from_file(&f.directory).unwrap();
        assert_eq!(saved.cubes.len(), 1);
        assert_eq!(saved.cubes[0].id, f.other.id);
        assert!(saved.wallets.is_empty());
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
    }

    #[tokio::test]
    async fn installed_wallet_and_journal_are_removed_without_touching_other_data() {
        let mut f = Fixture::new().await;
        f.install().await;
        let destination = f.destination();
        let journal = journal_path(&f.directory, f.operation_id());
        assert!(destination.join("coincubed.sqlite3").is_file());
        assert!(journal.is_file());
        let unrelated = f.directory.path().join("keep-me");
        fs::write(&unrelated, b"unrelated").unwrap();
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        let saved = Settings::from_file(&f.directory).unwrap();
        assert_eq!(saved.cubes.len(), 1);
        assert_eq!(saved.cubes[0].id, f.other.id);
        assert!(saved.wallets.is_empty());
        assert!(!destination.exists());
        assert!(!journal.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
    }

    /// Deleting a Cube right after locking it: its imported daemon may still
    /// be stopping in the wallet directory, so removal waits for that stop.
    #[tokio::test]
    async fn removal_waits_for_a_daemon_still_stopping_in_the_wallet_directory() {
        let mut f = Fixture::new().await;
        f.install().await;
        let destination = f.destination();
        let (release, released) = std::sync::mpsc::channel::<()>();
        // Registered as production does: the daemon config's canonical path.
        let registered = std::fs::canonicalize(&destination).unwrap();
        crate::daemon::stopping::run_in_background(Some(registered), move || {
            released.recv().unwrap();
        });
        let removal = remove_cube_import(&f.directory, &f.cube);
        tokio::pin!(removal);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), &mut removal)
                .await
                .is_err(),
            "removal must wait for the stopping daemon"
        );
        assert!(destination.join("coincubed.sqlite3").is_file());
        release.send(()).unwrap();
        removal.await.unwrap();
        assert!(!destination.exists());
    }

    /// A stop that outlasts the wait leaves every file and the Cube in place.
    #[tokio::test]
    async fn removal_refuses_while_the_daemon_is_still_stopping() {
        let mut f = Fixture::new().await;
        f.install().await;
        let destination = f.destination();
        let journal = journal_path(&f.directory, f.operation_id());
        let (release, released) = std::sync::mpsc::channel::<()>();
        // Registered as production does: the daemon config's canonical path.
        let registered = std::fs::canonicalize(&destination).unwrap();
        crate::daemon::stopping::run_in_background(Some(registered), move || {
            released.recv().unwrap();
        });
        let error =
            remove_cube_import_within(&f.directory, &f.cube, std::time::Duration::from_millis(50))
                .await
                .unwrap_err();
        assert_eq!(error, crate::daemon::stopping::STILL_STOPPING);
        assert!(destination.join("coincubed.sqlite3").is_file());
        assert!(journal.is_file());
        let saved = Settings::from_file(&f.directory).unwrap();
        assert!(saved.cubes.iter().any(|cube| cube.id == f.cube.id));
        release.send(()).unwrap();
        assert!(crate::daemon::stopping::wait_for(&destination));
    }

    #[tokio::test]
    async fn owner_verified_removal_works_with_corrupt_local_key_manifest() {
        let mut f = Fixture::new().await;
        f.install().await;
        let key_root = f
            .directory
            .path()
            .join("imported-local-keys")
            .join(f.operation_id().to_string());
        crate::dir::create_directory(&key_root).unwrap();
        fs::write(key_root.join("manifest.json"), b"{invalid").unwrap();
        fs::write(key_root.join("orphan.key"), b"synthetic ciphertext").unwrap();
        let unrelated = f.directory.path().join("keep-me");
        fs::write(&unrelated, b"unrelated").unwrap();

        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        assert!(!key_root.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
        assert_eq!(Settings::from_file(&f.directory).unwrap().cubes.len(), 1);
    }

    /// A BTCB2 Cube paired from a Bitcoin import has an attached wallet but
    /// can never hold imported local keys; deleting it must not consult the
    /// local-key repository, which refuses every BTCB2 scope.
    #[tokio::test]
    async fn installed_btcb2_import_is_removed() {
        let mut f = Fixture::on_chain(ChainId::BitcoinBlake2b).await;
        f.install().await;
        let destination = f.destination();
        let journal = journal_path(&f.directory, f.operation_id());
        assert!(destination.join("coincubed.sqlite3").is_file());
        let unrelated = f.directory.path().join("keep-me");
        fs::write(&unrelated, b"unrelated").unwrap();

        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        let saved = Settings::from_file(&f.directory).unwrap();
        assert_eq!(saved.cubes.len(), 1);
        assert_eq!(saved.cubes[0].id, f.other.id);
        assert!(saved.wallets.is_empty());
        assert!(!destination.exists());
        assert!(!journal.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
    }

    #[tokio::test]
    async fn btcb2_import_with_unexplained_key_storage_is_refused() {
        let mut f = Fixture::on_chain(ChainId::BitcoinBlake2b).await;
        f.install().await;
        let destination = f.destination();
        let key_root = f
            .directory
            .path()
            .join("imported-local-keys")
            .join(f.operation_id().to_string());
        crate::dir::create_directory(&key_root).unwrap();
        fs::write(key_root.join("orphan.key"), b"synthetic ciphertext").unwrap();

        let error = remove_cube_import(&f.directory, &f.cube).await.unwrap_err();
        assert!(
            error.contains("no files were removed"),
            "unexpected error: {}",
            error
        );
        assert!(key_root.join("orphan.key").is_file());
        assert!(destination.join("coincubed.sqlite3").is_file());
        let saved = Settings::from_file(&f.directory).unwrap();
        assert!(saved.cubes.iter().any(|cube| cube.id == f.cube.id));
    }

    #[tokio::test]
    async fn stale_retry_without_owner_does_not_remove_unclaimed_wallet_row() {
        let mut f = Fixture::new().await;
        f.install().await;
        let row = Settings::from_file(&f.directory).unwrap().wallets[0].clone();
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        settings::update_settings_file(&f.directory, |mut settings| {
            settings.wallets.push(row.clone());
            Some(settings)
        })
        .await
        .unwrap();
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        let saved = Settings::from_file(&f.directory).unwrap();
        assert_eq!(saved.wallets.len(), 1);
        assert_eq!(saved.wallets[0].wallet_id(), f.wallet.wallet_id.clone());
    }

    #[tokio::test]
    async fn staged_storage_is_removed_only_for_matching_owner() {
        let mut f = Fixture::new().await;
        f.install().await;
        let journal = read_journal(&journal_path(&f.directory, f.operation_id()))
            .unwrap()
            .unwrap();
        let stage = journal.stage_path(&f.directory);
        fs::rename(f.destination(), &stage).unwrap();
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        assert!(!stage.exists());
        assert!(!journal.path(&f.directory).exists());
    }

    #[tokio::test]
    async fn interrupted_cleanup_finishes_from_visible_cube() {
        let mut f = Fixture::new().await;
        f.install().await;
        fs::remove_dir_all(f.destination()).unwrap();
        fs::remove_file(journal_path(&f.directory, f.operation_id())).unwrap();
        assert!(Settings::from_file(&f.directory)
            .unwrap()
            .cubes
            .iter()
            .any(|cube| cube.id == f.cube.id));
        remove_cube_import(&f.directory, &f.cube).await.unwrap();
        assert!(!Settings::from_file(&f.directory)
            .unwrap()
            .cubes
            .iter()
            .any(|cube| cube.id == f.cube.id));
    }

    #[tokio::test]
    async fn existing_final_storage_without_journal_or_owner_blocks_deletion() {
        let mut f = Fixture::new().await;
        f.install().await;
        fs::remove_file(journal_path(&f.directory, f.operation_id())).unwrap();
        fs::remove_file(f.destination().join(OWNER_FILE)).unwrap();
        let before = fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap();
        assert!(remove_cube_import(&f.directory, &f.cube).await.is_err());
        assert_eq!(
            fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
            before
        );
        assert!(f.destination().join("coincubed.sqlite3").is_file());
    }

    #[tokio::test]
    async fn mismatched_owner_or_stale_operation_refuses_without_mutation() {
        let mut f = Fixture::new().await;
        f.install().await;
        let mut stale = f.cube.clone();
        stale.bitcoin_import.as_mut().unwrap().operation_id = Uuid::new_v4();
        assert!(remove_cube_import(&f.directory, &stale).await.is_err());
        let marker = f.destination().join(OWNER_FILE);
        let original = fs::read(&marker).unwrap();
        let mut forged: serde_json::Value = serde_json::from_slice(&original).unwrap();
        forged["cube_id"] = serde_json::json!(f.other.id.clone());
        fs::write(&marker, serde_json::to_vec(&forged).unwrap()).unwrap();
        let settings_before =
            fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap();
        assert!(remove_cube_import(&f.directory, &f.cube).await.is_err());
        assert_eq!(
            fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
            settings_before
        );
        assert!(f.destination().join("coincubed.sqlite3").is_file());
        fs::write(marker, original).unwrap();
    }

    #[tokio::test]
    async fn another_cube_referencing_the_wallet_prevents_deletion() {
        let mut f = Fixture::new().await;
        f.install().await;
        settings::update_settings_file(&f.directory, |mut settings| {
            let other = settings
                .cubes
                .iter_mut()
                .find(|cube| cube.id == f.other.id)
                .unwrap();
            other.bitcoin_import = Some(Box::new(BitcoinImport {
                operation_id: Uuid::new_v4(),
                stage: BitcoinImportStage::Ready {
                    wallet: f.wallet.clone(),
                },
                last_error: None,
            }));
            Some(settings)
        })
        .await
        .unwrap();
        let before = fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap();
        assert!(remove_cube_import(&f.directory, &f.cube).await.is_err());
        assert_eq!(
            fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
            before
        );
        assert!(f.destination().join("coincubed.sqlite3").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_storage_parent_is_refused_before_settings_change() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new().await;
        let outside = f.root.join("unrelated-data");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep-me"), b"unrelated").unwrap();
        let data = f.directory.path().join("data");
        fs::rename(&data, f.directory.path().join("original-data")).unwrap();
        symlink(&outside, &data).unwrap();
        let before = fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap();
        assert!(remove_cube_import(&f.directory, &f.cube).await.is_err());
        assert_eq!(
            fs::read(f.directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
            before
        );
        assert_eq!(fs::read(outside.join("keep-me")).unwrap(), b"unrelated");
    }
}
