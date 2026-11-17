//! Recoverable installation of an imported wallet into one explicitly named Cube.
//!
//! The journal and ownership marker contain public metadata only. A wallet
//! becomes visible only after complete staged storage has been published. An
//! interrupted operation can publish/attach the same storage again, never choose
//! another Cube or delete a directory not owned by this operation.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use async_fd_lock::LockWrite;
use coincube_core::wallet_descriptor::WalletDescriptor;
use coincubed::{config::Config, datadir::DataDirectory};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    app::settings::{
        self, BitcoinImportStage, ImportedWalletAttachment, Settings, SettingsError, WalletId,
        WalletSettings,
    },
    dir::NetworkDirectory,
    services::duress::DuressLocalState,
};

pub mod pairing;
mod removal;
pub use removal::remove_cube_import;

const OWNER_FILE: &str = "import-owner.json";
const CONFIG_FILE: &str = "daemon.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    cube_id: String,
    operation_id: Uuid,
    wallet: ImportedWalletAttachment,
    phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restored_address_state: Option<RestoredAddressState>,
    /// Immutable copied or authenticated Kit discovery minimum, initialized before publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restored_discovery_target: Option<u32>,
}

/// Durable high-water marks from a Recovery Kit; these are allocated indices,
/// not historical scan coverage. They must be restored before publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredAddressState {
    pub receive_index: u32,
    pub change_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    StorageReady,
    Published,
    Attached,
}

impl Journal {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported wallet import journal version.".into());
        }
        if self.restored_discovery_target.is_some_and(|target| {
            !(self.wallet.initial_lookahead..=10_000).contains(&target)
                || self.restored_address_state.is_none()
        }) {
            return Err("The copied wallet discovery range is invalid.".into());
        }
        Ok(())
    }

    fn path(&self, directory: &NetworkDirectory) -> PathBuf {
        journal_path(directory, self.operation_id)
    }
    fn stage_path(&self, directory: &NetworkDirectory) -> PathBuf {
        directory
            .path()
            .join("data")
            .join(format!(".import-{}", self.operation_id))
    }
    fn final_path(&self, directory: &NetworkDirectory) -> PathBuf {
        directory
            .coincubed_data_directory(&self.wallet.wallet_id)
            .path()
            .to_path_buf()
    }
    fn persist(&self, directory: &NetworkDirectory) -> Result<(), String> {
        self.validate()?;
        write_json(&self.path(directory), self)
    }
}

fn journal_path(directory: &NetworkDirectory, operation_id: Uuid) -> PathBuf {
    directory
        .path()
        .join("imports")
        .join(format!("{operation_id}.json"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing import storage directory")?;
    crate::dir::create_directory(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".import-write-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        sync_directory(parent)
    })();
    if tmp.exists() {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// Runs local-filesystem work on Tokio's blocking pool so a slow disk never
/// stalls the async executor that drives the UI.
pub(super) async fn blocking_fs<T, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("Filesystem task failed: {e}"))?
}

/// Discards the staging directory left by an earlier attempt of this exact
/// operation. Staged data owned by anything else is refused unless empty.
fn discard_owned_stage(stage: &Path, journal: &Journal) -> Result<(), String> {
    if !stage.exists() {
        return Ok(());
    }
    if owns_directory(stage, journal)? {
        // Only unpublished storage owned by this exact prepared operation
        // can be discarded and recreated. Never touch a final wallet.
        fs::remove_dir_all(stage).map_err(|e| e.to_string())?;
    } else if fs::read_dir(stage)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err(
            "The staging directory is not owned by this import; no files were changed.".into(),
        );
    }
    Ok(())
}

/// Removes this operation's staged data before its pending setup is abandoned.
fn remove_abandoned_stage(stage: &Path, journal: &Journal) -> Result<(), String> {
    if !stage.exists() {
        return Ok(());
    }
    if !owns_directory(stage, journal)? {
        return Err("The staged data is not owned by this import; no files were removed.".into());
    }
    fs::remove_dir_all(stage).map_err(|e| e.to_string())?;
    sync_directory(stage.parent().ok_or("Missing staging parent")?)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_journal(path: &Path) -> Result<Option<Journal>, String> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Journal = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid import journal: {e}"))?;
            value.validate()?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read import journal: {e}")),
    }
}

fn check_duress(directory: &NetworkDirectory) -> Result<(), String> {
    let root = directory
        .path()
        .parent()
        .ok_or("Missing Cube data directory")?;
    let state =
        DuressLocalState::load(root).map_err(|e| format!("Cannot check Duress Mode: {e}"))?;
    if state.enrolled || state.arming || state.active {
        return Err("Resolve Duress Mode in Settings before importing this wallet. Imported Bitcoin wallets do not yet support Duress Mode recovery.".into());
    }
    Ok(())
}

fn check_directory_chain(
    directory: &NetworkDirectory,
    chain: crate::chain::ChainId,
) -> Result<(), String> {
    if directory.path().file_name() != Some(std::ffi::OsStr::new(chain.dir_name())) {
        return Err("The import storage directory belongs to a different chain.".into());
    }
    Ok(())
}

fn check_cube(
    directory: &NetworkDirectory,
    settings: &Settings,
    journal: &Journal,
    allow_attached: bool,
) -> Result<(), String> {
    let cube = settings
        .cubes
        .iter()
        .find(|c| c.id == journal.cube_id)
        .ok_or("The Cube for this import no longer exists.")?;
    check_directory_chain(directory, cube.network)?;
    journal.wallet.validate(cube.network)?;
    if cube.vault_wallet_id.is_some() {
        return Err("This Cube already has a Vault.".into());
    }
    let intent = cube
        .bitcoin_import
        .as_ref()
        .ok_or("This Cube is not waiting for a wallet import.")?;
    if intent.operation_id != journal.operation_id {
        return Err("This import belongs to a different setup operation.".into());
    }
    if let Some(wallet) = intent.attachment() {
        if wallet != &journal.wallet {
            return Err("The pending import has changed. Review the wallet again.".into());
        }
    }
    if !allow_attached && intent.active_attachment().is_some() {
        return Err("This Cube already has an imported wallet.".into());
    }
    if settings.cubes.iter().any(|other| {
        other.id != cube.id
            && other
                .bitcoin_import
                .as_ref()
                .and_then(|v| v.attachment())
                .is_some_and(|w| w.wallet_id == journal.wallet.wallet_id)
    }) {
        return Err("This Bitcoin wallet is already attached to another Cube.".into());
    }
    Ok(())
}

async fn set_stage(
    directory: &NetworkDirectory,
    journal: &Journal,
    stage: BitcoinImportStage,
) -> Result<(), String> {
    settings::update_settings_file_checked(directory, |mut settings| {
        check_cube(directory, &settings, journal, true).map_err(SettingsError::WritingFile)?;
        let cube = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == journal.cube_id)
            .ok_or_else(|| SettingsError::WritingFile("Import Cube is missing".into()))?;
        let intent = cube
            .bitcoin_import
            .as_mut()
            .ok_or_else(|| SettingsError::WritingFile("Import intent is missing".into()))?;
        // A retry after successful attachment must not move a ready wallet back
        // to Installing or Syncing or reset its address allocations.
        if !intent.is_ready() {
            intent.stage = stage;
            intent.last_error = None;
        }
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())
}

fn owns_directory(path: &Path, journal: &Journal) -> Result<bool, String> {
    let Some(owner) = read_journal(&path.join(OWNER_FILE))? else {
        return Ok(false);
    };
    Ok(owner.cube_id == journal.cube_id
        && owner.operation_id == journal.operation_id
        && owner.wallet == journal.wallet
        && owner.restored_address_state == journal.restored_address_state
        && owner.restored_discovery_target == journal.restored_discovery_target)
}

fn verify_published(directory: &NetworkDirectory, journal: &Journal) -> Result<(), String> {
    let destination = journal.final_path(directory);
    if !owns_directory(&destination, journal)? {
        return Err(
            "The wallet directory belongs to another operation; no files were changed.".into(),
        );
    }
    let config =
        Config::from_file(Some(destination.join(CONFIG_FILE))).map_err(|e| e.to_string())?;
    if config.main_descriptor != WalletDescriptor::Imported(journal.wallet.descriptor.clone())
        || config.bitcoin_config.chain != journal.wallet.descriptor.chain()
        || config.import_birthday != Some(journal.wallet.birthday)
        || config.import_initial_lookahead != Some(journal.wallet.initial_lookahead)
        || config.data_directory().as_ref().map(|d| d.path()) != Some(destination.as_path())
        || !destination.join("coincubed.sqlite3").is_file()
    {
        return Err("The installed wallet does not match the import journal.".into());
    }
    if let Some(required) = journal.restored_address_state {
        let (receive, change) =
            coincubed::read_imported_address_state(&config, &DataDirectory::new(destination))
                .map_err(|e| e.to_string())?;
        if receive < required.receive_index || change < required.change_index {
            return Err("The restored wallet is missing its saved address reservations.".into());
        }
    }
    if let Some(required) = journal.restored_discovery_target {
        let state = coincubed::read_imported_claim_state(
            &config,
            &DataDirectory::new(journal.final_path(directory)),
        )
        .map_err(|e| e.to_string())?;
        if state.discovery_target < required {
            return Err("The installed wallet is missing its copied discovery range.".into());
        }
    }
    Ok(())
}

fn check_pending_paired_discovery(settings: &Settings, journal: &Journal) -> Result<(), String> {
    let cube = settings
        .cubes
        .iter()
        .find(|cube| cube.id == journal.cube_id)
        .ok_or("Import Cube is missing")?;
    if cube.imported_claim_source.is_some()
        && cube
            .bitcoin_import
            .as_ref()
            .is_some_and(|intent| !intent.is_ready())
        && journal.restored_discovery_target.is_none()
    {
        return Err(
            "The pending paired import is missing its saved discovery requirement; no wallet was attached."
                .into(),
        );
    }
    Ok(())
}

async fn finish(directory: &NetworkDirectory, journal: &mut Journal) -> Result<WalletId, String> {
    check_duress(directory)?;
    let saved = Settings::from_file(directory).map_err(|e| e.to_string())?;
    check_cube(directory, &saved, journal, true)?;
    check_pending_paired_discovery(&saved, journal)?;
    let stage = journal.stage_path(directory);
    let destination = journal.final_path(directory);
    if destination.exists() {
        verify_published(directory, journal)?;
    } else {
        if journal.phase == Phase::Prepared {
            return Err(
                "Wallet installation was interrupted. Review and confirm this import again.".into(),
            );
        }
        if !owns_directory(&stage, journal)? {
            return Err(
                "The staged wallet is missing its ownership record; no files were changed.".into(),
            );
        }
        fs::rename(&stage, &destination)
            .map_err(|e| format!("Cannot publish imported wallet: {e}"))?;
        sync_directory(
            destination
                .parent()
                .ok_or("Missing wallet parent directory")?,
        )?;
        verify_published(directory, journal)?;
    }
    journal.phase = Phase::Published;
    journal.persist(directory)?;
    // One settings transaction attaches the exact Cube and registers the local
    // wallet. This never uses the legacy 'first empty Cube' fallback.
    settings::update_settings_file_checked(directory, |mut settings| {
        check_cube(directory, &settings, journal, true).map_err(SettingsError::WritingFile)?;
        check_pending_paired_discovery(&settings, journal).map_err(SettingsError::WritingFile)?;
        let cube = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == journal.cube_id)
            .ok_or_else(|| SettingsError::WritingFile("Import Cube is missing".into()))?;
        let intent = cube
            .bitcoin_import
            .as_mut()
            .ok_or_else(|| SettingsError::WritingFile("Import intent is missing".into()))?;
        let newly_attached = intent.active_attachment().is_none();
        if !intent.is_ready() {
            intent.stage = BitcoinImportStage::Syncing {
                wallet: journal.wallet.clone(),
            };
        }
        intent.last_error = None;
        // Seed-only creation Kits become stale only on first attachment. A
        // restart must preserve a Kit updated after the import completed.
        if newly_attached {
            cube.recovery_kit_last_backed_up_descriptor_fingerprint = None;
            cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint = None;
        }
        if !settings
            .wallets
            .iter()
            .any(|w| w.wallet_id() == journal.wallet.wallet_id)
        {
            settings.wallets.push(WalletSettings {
                name: "Imported Bitcoin wallet".into(),
                alias: None,
                descriptor_checksum: journal.wallet.wallet_id.descriptor_checksum.clone(),
                pinned_at: journal.wallet.wallet_id.timestamp,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth: None,
                start_internal_bitcoind: Some(false),
                pending_rescan: None,
                keychain_keys_recorded: false,
            });
        }
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())?;
    journal.phase = Phase::Attached;
    journal.persist(directory)?;
    Ok(journal.wallet.wallet_id.clone())
}

/// Install or retry one validated public wallet. The config must point at its
/// final directory; initialization happens privately in a separate stage.
pub async fn install(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
    wallet: ImportedWalletAttachment,
    config: Config,
) -> Result<WalletId, String> {
    install_inner(directory, cube_id, operation_id, wallet, config, None, None).await
}

/// Recovery installation uses the same journal, adding address reservations
/// before storage becomes visible to the GUI or a running daemon.
pub async fn install_restored(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
    wallet: ImportedWalletAttachment,
    config: Config,
    address_state: RestoredAddressState,
) -> Result<WalletId, String> {
    install_inner(
        directory,
        cube_id,
        operation_id,
        wallet,
        config,
        Some(address_state),
        None,
    )
    .await
}

/// A paired Claim copies historical discovery independently of allocated
/// addresses. Both must be durable in unpublished storage before attachment.
async fn install_restored_with_discovery(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
    wallet: ImportedWalletAttachment,
    config: Config,
    address_state: RestoredAddressState,
    discovery_target: u32,
) -> Result<WalletId, String> {
    install_inner(
        directory,
        cube_id,
        operation_id,
        wallet,
        config,
        Some(address_state),
        Some(discovery_target),
    )
    .await
}

async fn install_inner(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
    wallet: ImportedWalletAttachment,
    config: Config,
    restored_address_state: Option<RestoredAddressState>,
    restored_discovery_target: Option<u32>,
) -> Result<WalletId, String> {
    directory.init().map_err(|e| e.to_string())?;
    let _lock = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.path().join("wallet-import.lock"))
        .await
        .map_err(|e| e.to_string())?
        .lock_write()
        .await
        .map_err(|e| format!("Cannot lock wallet import: {e:?}"))?;
    check_duress(directory)?;
    let mut journal = Journal {
        version: 1,
        cube_id: cube_id.into(),
        operation_id,
        wallet,
        phase: Phase::Prepared,
        restored_address_state,
        restored_discovery_target,
    };
    journal.validate()?;
    check_cube(
        directory,
        &Settings::from_file(directory).map_err(|e| e.to_string())?,
        &journal,
        true,
    )?;
    let destination = journal.final_path(directory);
    if config.main_descriptor != WalletDescriptor::Imported(journal.wallet.descriptor.clone())
        || config.data_directory().as_ref().map(|d| d.path()) != Some(destination.as_path())
    {
        return Err(
            "The import configuration does not match this wallet and its storage directory.".into(),
        );
    }
    journal.wallet.validate(config.bitcoin_config.chain)?;
    if let Some(prior) = read_journal(&journal.path(directory))? {
        if prior.cube_id != journal.cube_id
            || prior.operation_id != operation_id
            || prior.wallet != journal.wallet
            || (journal.restored_address_state.is_some()
                && prior.restored_address_state != journal.restored_address_state)
            || (journal.restored_discovery_target.is_some()
                && prior.restored_discovery_target != journal.restored_discovery_target)
        {
            return Err("This import operation already names a different wallet.".into());
        }
        journal = prior;
        if journal.phase != Phase::Prepared || destination.exists() {
            return finish(directory, &mut journal).await;
        }
    } else if destination.exists() {
        return Err("A wallet already exists at this destination. No files were changed.".into());
    }
    check_pending_paired_discovery(
        &Settings::from_file(directory).map_err(|e| e.to_string())?,
        &journal,
    )?;
    journal.persist(directory)?;
    set_stage(
        directory,
        &journal,
        BitcoinImportStage::Installing {
            wallet: journal.wallet.clone(),
        },
    )
    .await?;
    let stage = journal.stage_path(directory);
    {
        let (stage, journal) = (stage.clone(), journal.clone());
        blocking_fs(move || discard_owned_stage(&stage, &journal)).await?;
    }
    crate::dir::create_directory(&stage).map_err(|e| e.to_string())?;
    write_json(&stage.join(OWNER_FILE), &journal)?;
    coincubed::create_imported_storage(
        &config,
        &DataDirectory::new(stage.clone()),
        journal.wallet.birthday,
        journal.wallet.initial_lookahead,
    )
    .map_err(|e| e.to_string())?;
    if let Some(state) = journal.restored_address_state {
        // The initializer writes scan fields into the saved configuration.
        let saved_config =
            Config::from_file(Some(stage.join(CONFIG_FILE))).map_err(|e| e.to_string())?;
        coincubed::restore_imported_address_state(
            &saved_config,
            &DataDirectory::new(stage.clone()),
            state.receive_index,
            state.change_index,
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(target) = journal.restored_discovery_target {
        let saved_config =
            Config::from_file(Some(stage.join(CONFIG_FILE))).map_err(|e| e.to_string())?;
        coincubed::restore_imported_discovery_target(
            &saved_config,
            &DataDirectory::new(stage.clone()),
            target,
        )
        .map_err(|e| e.to_string())?;
    }
    sync_directory(&stage)?;
    journal.phase = Phase::StorageReady;
    journal.persist(directory)?;
    finish(directory, &mut journal).await
}

/// The authenticated recovery route whose backup record was just restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoredKitSource {
    Password,
    /// Password-v3 public restore leaves key-backup completion unset; local
    /// signer admission is handled by the restore coordinator.
    PasswordV3,
    OwnerKeychain,
}

/// Restore the public-wallet half of an authenticated Recovery Kit into its
/// exact Cube. The caller restores/unlocks the Cube seed through the normal
/// installer; it is never supplied to this wallet's spending code.
pub async fn restore_from_kit(
    directory: &NetworkDirectory,
    cube_id: &str,
    blob: &crate::services::recovery::ImportedDescriptorBlob,
    backend: coincubed::config::BitcoinBackend,
    source: RestoredKitSource,
) -> Result<settings::CubeSettings, String> {
    let chain = blob.wallet.descriptor.chain();
    blob.validate(chain)?;
    check_directory_chain(directory, chain)?;
    check_duress(directory)?;
    if cube_id != blob.cube.uuid {
        return Err("This imported wallet Recovery Kit belongs to another Cube.".into());
    }
    let wallet = ImportedWalletAttachment::new(
        blob.wallet.descriptor.clone(),
        chain,
        blob.wallet.birthday,
        blob.wallet.initial_lookahead,
    )?;
    settings::update_settings_file_checked(directory, |mut settings| {
        if settings
            .cubes
            .iter()
            .filter(|cube| cube.id == cube_id)
            .count()
            != 1
        {
            return Err(SettingsError::WritingFile(
                "Duplicate or missing restore Cube identity.".into(),
            ));
        }
        if let Some(metadata) = &blob.signer_metadata {
            let matches = settings
                .wallets
                .iter()
                .filter(|w| w.wallet_id() == wallet.wallet_id)
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(SettingsError::WritingFile(
                    "Duplicate restored wallet identity.".into(),
                ));
            }
            if let Some(prior) = matches.first() {
                let prior_signers = serde_json::to_value((
                    &prior.keys,
                    &prior.hardware_wallets,
                    prior.keychain_keys_recorded,
                ))
                .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
                let restored_signers = serde_json::to_value((
                    &metadata.keys,
                    &metadata.hardware_wallets,
                    metadata.keychain_keys_recorded,
                ))
                .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
                if (!prior.keys.is_empty()
                    || !prior.hardware_wallets.is_empty()
                    || prior.keychain_keys_recorded)
                    && prior_signers != restored_signers
                {
                    return Err(SettingsError::WritingFile(
                        "Existing imported signer metadata conflicts with this Recovery Kit."
                            .into(),
                    ));
                }
            }
        }
        let cube = settings
            .cubes
            .iter_mut()
            .find(|cube| cube.id == cube_id)
            .ok_or_else(|| {
                SettingsError::WritingFile("The Cube to restore no longer exists.".into())
            })?;
        if cube.network != chain || cube.vault_wallet_id.is_some() {
            return Err(SettingsError::WritingFile(
                "This Cube already has a Vault or belongs to another chain.".into(),
            ));
        }
        if cube.imported_claim_source.is_some()
            && cube.imported_claim_source != blob.imported_claim_source
        {
            return Err(SettingsError::WritingFile(
                "This Cube already has another imported Claim source.".into(),
            ));
        }
        cube.imported_claim_source = blob.imported_claim_source.clone();
        match &cube.bitcoin_import {
            Some(intent)
                if blob
                    .import_operation_id
                    .is_some_and(|id| id != intent.operation_id) =>
            {
                return Err(SettingsError::WritingFile(
                    "This Cube already belongs to another import operation.".into(),
                ));
            }
            Some(intent)
                if intent
                    .attachment()
                    .is_some_and(|present| present != &wallet) =>
            {
                return Err(SettingsError::WritingFile(
                    "This Cube is already importing another Bitcoin wallet.".into(),
                ));
            }
            None => {
                let mut intent = settings::BitcoinImport::pending();
                if let Some(operation_id) = blob.import_operation_id {
                    intent.operation_id = operation_id;
                }
                cube.bitcoin_import = Some(Box::new(intent));
            }
            _ => (),
        }
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())?;
    let operation_id = Settings::from_file(directory)
        .map_err(|e| e.to_string())?
        .cubes
        .into_iter()
        .find(|cube| cube.id == cube_id)
        .and_then(|cube| cube.bitcoin_import)
        .ok_or("The restore operation disappeared.")?
        .operation_id;
    let config = Config::new(
        coincubed::config::BitcoinConfig::new(chain, std::time::Duration::from_secs(10)),
        Some(backend),
        log::LevelFilter::Info,
        WalletDescriptor::Imported(blob.wallet.descriptor.clone()),
        directory.coincubed_data_directory(&wallet.wallet_id),
    );
    let address_state = RestoredAddressState {
        receive_index: blob.wallet.receive_index,
        change_index: blob.wallet.change_index,
    };
    let discovery_target = if blob.imported_claim_source.is_some() {
        // The authenticated Kit establishes this minimum, independently of
        // restored allocation high-water marks. Preserve any earlier journal's
        // stronger copied requirement on retry; never replace it with the Kit's
        // initial lookahead. Legacy pending records without a minimum fail closed
        // in the common installer, while completed imports retain compatibility.
        match read_journal(&journal_path(directory, operation_id))? {
            Some(prior) => prior.restored_discovery_target,
            None => Some(blob.wallet.initial_lookahead),
        }
    } else {
        None
    };
    if let Some(target) = discovery_target {
        install_restored_with_discovery(
            directory,
            cube_id,
            operation_id,
            wallet.clone(),
            config,
            address_state,
            target,
        )
        .await?;
    } else {
        install_restored(
            directory,
            cube_id,
            operation_id,
            wallet.clone(),
            config,
            address_state,
        )
        .await?;
    }
    if let Some(metadata) = &blob.signer_metadata {
        settings::update_settings_file_checked(directory, |mut saved| {
            let entry = saved
                .wallets
                .iter_mut()
                .find(|w| w.wallet_id() == wallet.wallet_id)
                .ok_or_else(|| {
                    SettingsError::WritingFile("Restored imported metadata is missing.".into())
                })?;
            *entry = metadata.clone();
            Ok(Some(saved))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let fingerprint = crate::services::recovery::imported_descriptor_blob_fingerprint(blob)
        .ok_or("Could not fingerprint the restored imported wallet.")?;
    settings::update_settings_file_checked(directory, |mut settings| {
        let cube = settings
            .cubes
            .iter_mut()
            .find(|cube| cube.id == cube_id)
            .ok_or_else(|| SettingsError::WritingFile("The restored Cube disappeared.".into()))?;
        if !cube.bitcoin_import.as_ref().is_some_and(|intent| {
            intent.operation_id == operation_id && intent.active_attachment() == Some(&wallet)
        }) {
            return Err(SettingsError::WritingFile(
                "The Cube's imported wallet changed during recovery.".into(),
            ));
        }
        match source {
            RestoredKitSource::Password => {
                cube.recovery_kit_password_backed_up = true;
                cube.recovery_kit_last_backed_up_descriptor_fingerprint = Some(fingerprint.clone());
            }
            RestoredKitSource::PasswordV3 => {
                cube.recovery_kit_password_backed_up = false;
                cube.recovery_kit_last_backed_up_descriptor_fingerprint = None;
            }
            RestoredKitSource::OwnerKeychain => {
                cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint =
                    Some(fingerprint.clone());
            }
        }
        if let Some(saved_wallet) = settings
            .wallets
            .iter_mut()
            .find(|entry| entry.wallet_id() == wallet.wallet_id)
        {
            saved_wallet.name = blob.wallet.name.clone();
        }
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())?;
    Settings::from_file(directory)
        .map_err(|e| e.to_string())?
        .cubes
        .into_iter()
        .find(|cube| cube.id == cube_id)
        .ok_or_else(|| "The restored Cube disappeared.".into())
}

/// Resume publication/attachment after restart. A Prepared journal needs the
/// review screen to supply configuration again; it is never mistaken for Ready.
pub async fn resume(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
) -> Result<Option<WalletId>, String> {
    directory.init().map_err(|e| e.to_string())?;
    let _lock = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.path().join("wallet-import.lock"))
        .await
        .map_err(|e| e.to_string())?
        .lock_write()
        .await
        .map_err(|e| format!("Cannot lock wallet import: {e:?}"))?;
    let Some(mut journal) = read_journal(&journal_path(directory, operation_id))? else {
        return Ok(None);
    };
    if journal.cube_id != cube_id || journal.operation_id != operation_id {
        return Err("This import journal belongs to a different Cube or setup operation.".into());
    }
    check_cube(
        directory,
        &Settings::from_file(directory).map_err(|e| e.to_string())?,
        &journal,
        true,
    )?;
    if journal.phase == Phase::Prepared && !journal.final_path(directory).exists() {
        return Ok(None);
    }
    finish(directory, &mut journal).await.map(Some)
}

/// Explicitly abandon setup before a wallet has been published. Ordinary
/// Cancel does not call this: it leaves the Cube's resumable import intact.
/// Published/attached wallets require the normal wallet-removal flow instead.
pub async fn abandon_pending(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
) -> Result<(), String> {
    let _lock = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.path().join("wallet-import.lock"))
        .await
        .map_err(|e| e.to_string())?
        .lock_write()
        .await
        .map_err(|e| format!("Cannot lock wallet import: {e:?}"))?;
    let current = Settings::from_file(directory).map_err(|e| e.to_string())?;
    let cube = current
        .cubes
        .iter()
        .find(|c| c.id == cube_id)
        .ok_or("The import Cube no longer exists.")?;
    let intent = cube
        .bitcoin_import
        .as_ref()
        .ok_or("This Cube has no pending import.")?;
    if intent.operation_id != operation_id || !intent.is_pending() || cube.vault_wallet_id.is_some()
    {
        return Err("This import can no longer be abandoned as an unfinished setup.".into());
    }
    let path = journal_path(directory, operation_id);
    if let Some(journal) = read_journal(&path)? {
        check_cube(directory, &current, &journal, false)?;
        if journal.cube_id != cube_id
            || journal.operation_id != operation_id
            || journal.final_path(directory).exists()
        {
            return Err(
                "This wallet has already been installed. Resume it before changing its attachment."
                    .into(),
            );
        }
        let stage = journal.stage_path(directory);
        blocking_fs(move || remove_abandoned_stage(&stage, &journal)).await?;
    }
    settings::update_settings_file_checked(directory, |mut settings| {
        let cube = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == cube_id)
            .ok_or_else(|| {
                SettingsError::WritingFile("The import Cube no longer exists.".into())
            })?;
        if !cube
            .bitcoin_import
            .as_ref()
            .is_some_and(|intent| intent.operation_id == operation_id && intent.is_pending())
            || cube.vault_wallet_id.is_some()
        {
            return Err(SettingsError::WritingFile(
                "This Cube's wallet setup changed. Nothing was detached.".into(),
            ));
        }
        cube.bitcoin_import = None;
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())?;
    match fs::remove_file(&path) {
        Ok(()) => sync_directory(path.parent().ok_or("Missing journal parent")?)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    Ok(())
}

/// Whether the daemon's report shows the imported wallet's history scan
/// complete. The first completion is strict: synced, no scan or rescan under
/// way and no failure. A wallet `already_ready` stays ready through what a
/// Vault also lives through (a failing poll, a catch-up sync, a background
/// scan) and leaves it only when its history is incomplete again, as after a
/// wider address search, or on chain divergence or a refused reorg.
pub fn history_scan_complete(
    info: &coincubed::commands::GetInfoResult,
    already_ready: bool,
) -> bool {
    let Some(status) = info.imported_wallet.as_ref() else {
        return false;
    };
    let history = status.history_complete
        && status.receive_scanned >= status.discovery_target
        && status.change_scanned >= status.discovery_target
        && !info.chain_divergence
        && info.refused_reorg_depth.is_none();
    history
        && (already_ready
            || (info.sync.is_finite()
                && info.sync >= 1.0
                && info.rescan_progress.is_none()
                && !info.history_sync.full_scan_in_progress
                && info.history_sync.last_failure.is_none()))
}

/// Reconcile durable setup readiness from a daemon positively identified as
/// this Cube's wallet. Current backend errors remain separate UI freshness
/// information; a different wallet/session cannot complete this operation.
pub async fn reconcile_sync(
    directory: &NetworkDirectory,
    cube_id: &str,
    operation_id: Uuid,
    info: &coincubed::commands::GetInfoResult,
) -> Result<(), String> {
    let status = info
        .imported_wallet
        .as_ref()
        .ok_or("The daemon did not report imported wallet synchronization status.")?;
    let imported = match &info.descriptors.main {
        WalletDescriptor::Imported(descriptor) => descriptor,
        WalletDescriptor::Vault(_) => {
            return Err("This daemon is running a Vault, not the imported Bitcoin wallet.".into())
        }
    };
    if info.chain_id != Some(status.chain_id)
        || info.network != status.chain_id.bitcoin_network()
        || imported
            .identity(status.chain_id)
            .map_err(|e| e.to_string())?
            .to_string()
            != status.wallet_identity
    {
        return Err(
            "The daemon's wallet identity or chain does not match its synchronization report."
                .into(),
        );
    }
    check_directory_chain(directory, status.chain_id)?;
    let update = |settings: &mut Settings| -> Result<bool, String> {
        let cube = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == cube_id)
            .ok_or("The import Cube no longer exists.")?;
        if cube.vault_wallet_id.is_some() {
            return Err("This Cube already has a Vault.".into());
        }
        let intent = cube
            .bitcoin_import
            .as_mut()
            .ok_or("The import operation no longer exists.")?;
        let wallet = intent
            .active_attachment()
            .ok_or("The imported wallet is not installed yet.")?
            .clone();
        wallet.validate(cube.network)?;
        if intent.operation_id != operation_id
            || cube.network != status.chain_id
            || wallet.descriptor != *imported
            || status.birthday != wallet.birthday
            || status.initial_lookahead < wallet.initial_lookahead
            || status.discovery_target < status.initial_lookahead
            || status.discovery_target > 10_000
        {
            return Err(
                "This synchronization result belongs to another wallet or setup operation.".into(),
            );
        }
        let stage = if history_scan_complete(info, intent.is_ready()) {
            BitcoinImportStage::Ready { wallet }
        } else {
            BitcoinImportStage::Syncing { wallet }
        };
        let changed = intent.stage != stage;
        intent.stage = stage;
        Ok(changed)
    };
    // A refresh with unchanged readiness should not rewrite settings. Validate
    // identity even on this fast path; returning None from the settings writer
    // would delete the settings file, not skip a write.
    let mut snapshot = Settings::from_file(directory).map_err(|e| e.to_string())?;
    if !update(&mut snapshot)? {
        return Ok(());
    }
    settings::update_settings_file_checked(directory, |mut settings| {
        // Revalidate against the state held under the settings write lock.
        update(&mut settings).map_err(SettingsError::WritingFile)?;
        Ok(Some(settings))
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_pair_tests {
    use super::*;
    use crate::{
        app::settings::{bitcoin_import::ImportedClaimPair, CubeSettings, KeySetting},
        chain::ChainId,
        dir::CoincubeDirectory,
        hw::HardwareWalletConfig,
        services::recovery::{
            decrypt_wallet_descriptor_blob, encrypt, ImportedDescriptorBlob,
            ImportedDescriptorBlobWallet, KdfParams, RecoveredWalletBlob,
        },
    };
    use coincube_core::{
        miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
            Network,
        },
        wallet_descriptor::ImportedWalletDescriptor,
    };
    use std::str::FromStr;

    struct RestoreRoot(CoincubeDirectory);
    impl RestoreRoot {
        fn new() -> Self {
            Self(CoincubeDirectory::new(
                std::env::temp_dir().join(format!("imported-pair-recovery-{}", Uuid::new_v4())),
            ))
        }
    }
    impl Drop for RestoreRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.path());
        }
    }

    fn kits() -> (ImportedDescriptorBlob, ImportedDescriptorBlob) {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[73; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84h/0h/0h]{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let source_wallet =
            ImportedWalletAttachment::new(descriptor.clone(), ChainId::Bitcoin, 0, 20).unwrap();
        let cube_id = Uuid::new_v4().to_string();
        let operation_id = Uuid::new_v4();
        let metadata = WalletSettings {
            name: "Synthetic source".into(),
            alias: None,
            descriptor_checksum: source_wallet.wallet_id.descriptor_checksum.clone(),
            pinned_at: None,
            keys: vec![KeySetting {
                name: "Synthetic external device".into(),
                master_fingerprint: master.fingerprint(&secp),
                provider_key: None,
                is_border_wallet: false,
                grid_seed_source: None,
                replay_protected: Some(true),
                keychain_key_id: None,
            }],
            hardware_wallets: vec![HardwareWalletConfig {
                kind: "synthetic-device".into(),
                fingerprint: master.fingerprint(&secp),
                token: hex::encode([17; 32]),
            }],
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: true,
        };
        let source = ImportedDescriptorBlob::new_with_pair(
            cube_id.clone(),
            ChainId::Bitcoin,
            ImportedDescriptorBlobWallet {
                name: metadata.name.clone(),
                descriptor,
                birthday: 0,
                initial_lookahead: 20,
                receive_index: 7,
                change_index: 3,
            },
            None,
            Some(metadata.clone()),
            Some(operation_id),
        )
        .unwrap();
        let target_id = pairing::target_cube_id(&cube_id);
        let fork_descriptor = source.wallet.descriptor.paired_blake2b().unwrap();
        let mut target_metadata = metadata;
        target_metadata.name = "Synthetic fork".into();
        target_metadata.descriptor_checksum =
            WalletId::for_imported(&fork_descriptor, ChainId::BitcoinBlake2b)
                .unwrap()
                .descriptor_checksum;
        let target = ImportedDescriptorBlob::new_with_pair(
            target_id.clone(),
            ChainId::BitcoinBlake2b,
            ImportedDescriptorBlobWallet {
                name: target_metadata.name.clone(),
                descriptor: fork_descriptor,
                birthday: 0,
                initial_lookahead: 20,
                receive_index: 11,
                change_index: 5,
            },
            Some(Box::new(ImportedClaimPair {
                source_cube_id: cube_id,
                source_operation_id: operation_id,
                source_wallet,
            })),
            Some(target_metadata),
            Some(Uuid::parse_str(&target_id).unwrap()),
        )
        .unwrap();
        (source, target)
    }

    fn encrypted_roundtrip(blob: &ImportedDescriptorBlob) -> ImportedDescriptorBlob {
        let password = zeroize::Zeroizing::new("synthetic recovery test password".to_owned());
        let ciphertext = encrypt(
            &serde_json::to_vec(blob).unwrap(),
            &password,
            KdfParams {
                memory_kib: 512,
                t_cost: 1,
                p_cost: 1,
            },
            "42",
        )
        .unwrap();
        let RecoveredWalletBlob::Imported(restored) =
            decrypt_wallet_descriptor_blob(&ciphertext, &password, "42").unwrap()
        else {
            panic!("Expected an imported wallet Kit");
        };
        restored
    }

    fn backend() -> coincubed::config::BitcoinBackend {
        coincubed::config::BitcoinBackend::Electrum(coincubed::config::ElectrumConfig {
            addr: "tcp://127.0.0.1:50001".into(),
            validate_domain: true,
        })
    }

    async fn seed_only_cube(root: &CoincubeDirectory, blob: &ImportedDescriptorBlob) {
        let directory = root.network_directory(blob.wallet.descriptor.chain());
        directory.init().unwrap();
        let mut cube = CubeSettings::new_with_raw_id(
            blob.cube.uuid.clone(),
            blob.wallet.name.clone(),
            blob.wallet.descriptor.chain(),
        );
        // Synthetic Cube unlock identity, deliberately distinct from the imported key.
        cube.master_signer_fingerprint = Some([1, 2, 3, 4].into());
        settings::update_settings_file(&directory, |mut saved| {
            saved.cubes.push(cube);
            Some(saved)
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn password_v3_public_restore_does_not_claim_key_backup_complete() {
        let (blob, _) = kits();
        let root = RestoreRoot::new();
        seed_only_cube(&root.0, &blob).await;
        let directory = root.0.network_directory(ChainId::Bitcoin);
        let cube = restore_from_kit(
            &directory,
            &blob.cube.uuid,
            &blob,
            backend(),
            RestoredKitSource::PasswordV3,
        )
        .await
        .unwrap();
        assert!(cube
            .bitcoin_import
            .as_ref()
            .unwrap()
            .active_attachment()
            .is_some());
        assert!(!cube.recovery_kit_password_backed_up);
        assert!(cube
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .is_none());
    }

    #[tokio::test]
    async fn generic_pending_pair_resume_refuses_missing_discovery_requirement() {
        let (_, blob) = kits();
        for phase in [Phase::StorageReady, Phase::Published] {
            let root = RestoreRoot::new();
            seed_only_cube(&root.0, &blob).await;
            let directory = root.0.network_directory(ChainId::BitcoinBlake2b);
            let cube = restore_from_kit(
                &directory,
                &blob.cube.uuid,
                &blob,
                backend(),
                RestoredKitSource::Password,
            )
            .await
            .unwrap();
            let intent = cube.bitcoin_import.unwrap();
            let wallet = intent.active_attachment().unwrap().clone();
            let mut journal = read_journal(&journal_path(&directory, intent.operation_id))
                .unwrap()
                .unwrap();
            let data = directory.coincubed_data_directory(&wallet.wallet_id);
            let config = Config::from_file(Some(data.path().join(CONFIG_FILE))).unwrap();
            journal.restored_discovery_target = None;
            journal.phase = phase;
            journal.persist(&directory).unwrap();
            write_json(&data.path().join(OWNER_FILE), &journal).unwrap();
            let storage = if phase == Phase::StorageReady {
                fs::rename(data.path(), journal.stage_path(&directory)).unwrap();
                journal.stage_path(&directory)
            } else {
                data.path().to_path_buf()
            };
            settings::update_settings_file(&directory, |mut saved| {
                saved.cubes[0].bitcoin_import.as_mut().unwrap().stage =
                    BitcoinImportStage::Installing {
                        wallet: wallet.clone(),
                    };
                Some(saved)
            })
            .await
            .unwrap();
            let settings_path = directory.path().join(settings::SETTINGS_FILE_NAME);
            let before_settings = fs::read(&settings_path).unwrap();
            let before_journal = fs::read(journal.path(&directory)).unwrap();
            let before_db = fs::read(storage.join("coincubed.sqlite3")).unwrap();
            let before_owner = fs::read(storage.join(OWNER_FILE)).unwrap();
            let error = resume(&directory, &blob.cube.uuid, intent.operation_id)
                .await
                .unwrap_err();
            assert!(error.contains("saved discovery requirement"), "{}", error);
            for restored in [false, true] {
                let result = if restored {
                    install_restored(
                        &directory,
                        &blob.cube.uuid,
                        intent.operation_id,
                        wallet.clone(),
                        config.clone(),
                        journal.restored_address_state.unwrap(),
                    )
                    .await
                } else {
                    install(
                        &directory,
                        &blob.cube.uuid,
                        intent.operation_id,
                        wallet.clone(),
                        config.clone(),
                    )
                    .await
                };
                assert!(result.unwrap_err().contains("saved discovery requirement"));
            }
            // Re-presenting a Kit must not silently manufacture the missing
            // contract for an old pending operation either.
            assert!(restore_from_kit(
                &directory,
                &blob.cube.uuid,
                &blob,
                backend(),
                RestoredKitSource::Password,
            )
            .await
            .unwrap_err()
            .contains("saved discovery requirement"));
            assert_eq!(fs::read(&settings_path).unwrap(), before_settings);
            assert_eq!(fs::read(journal.path(&directory)).unwrap(), before_journal);
            assert_eq!(
                fs::read(storage.join("coincubed.sqlite3")).unwrap(),
                before_db
            );
            assert_eq!(fs::read(storage.join(OWNER_FILE)).unwrap(), before_owner);
            assert_eq!(data.path().exists(), phase == Phase::Published);
        }
    }

    #[tokio::test]
    async fn authenticated_paired_kit_restore_resumes_and_preserves_discovery_requirement() {
        let (_, blob) = kits();
        let root = RestoreRoot::new();
        seed_only_cube(&root.0, &blob).await;
        let directory = root.0.network_directory(ChainId::BitcoinBlake2b);
        let cube = restore_from_kit(
            &directory,
            &blob.cube.uuid,
            &blob,
            backend(),
            RestoredKitSource::Password,
        )
        .await
        .unwrap();
        let intent = cube.bitcoin_import.unwrap();
        let wallet = intent.active_attachment().unwrap().clone();
        let mut journal = read_journal(&journal_path(&directory, intent.operation_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            journal.restored_discovery_target,
            Some(blob.wallet.initial_lookahead)
        );
        let data = directory.coincubed_data_directory(&wallet.wallet_id);
        let config = Config::from_file(Some(data.path().join(CONFIG_FILE))).unwrap();
        // A crash after staged initialization can resume through the generic
        // restart path using the already authenticated durable minimum.
        fs::rename(data.path(), journal.stage_path(&directory)).unwrap();
        journal.phase = Phase::StorageReady;
        journal.persist(&directory).unwrap();
        settings::update_settings_file(&directory, |mut saved| {
            saved.cubes[0].bitcoin_import.as_mut().unwrap().stage =
                BitcoinImportStage::Installing {
                    wallet: wallet.clone(),
                };
            Some(saved)
        })
        .await
        .unwrap();
        assert_eq!(
            resume(&directory, &blob.cube.uuid, intent.operation_id)
                .await
                .unwrap(),
            Some(wallet.wallet_id.clone())
        );
        assert_eq!(
            coincubed::read_imported_address_state(&config, &data).unwrap(),
            (blob.wallet.receive_index, blob.wallet.change_index)
        );
        // A Kit retry must also retain a stronger range already established by
        // source copying, rather than replacing 650 with initial lookahead 20.
        coincubed::expand_imported_discovery(&config, &data, 650).unwrap();
        journal.restored_discovery_target = Some(650);
        journal.phase = Phase::StorageReady;
        journal.persist(&directory).unwrap();
        write_json(&data.path().join(OWNER_FILE), &journal).unwrap();
        fs::rename(data.path(), journal.stage_path(&directory)).unwrap();
        settings::update_settings_file(&directory, |mut saved| {
            saved.cubes[0].bitcoin_import.as_mut().unwrap().stage =
                BitcoinImportStage::Installing {
                    wallet: wallet.clone(),
                };
            Some(saved)
        })
        .await
        .unwrap();
        restore_from_kit(
            &directory,
            &blob.cube.uuid,
            &blob,
            backend(),
            RestoredKitSource::Password,
        )
        .await
        .unwrap();
        assert_eq!(
            read_journal(&journal.path(&directory))
                .unwrap()
                .unwrap()
                .restored_discovery_target,
            Some(650)
        );
        assert_eq!(
            coincubed::read_imported_claim_state(&config, &data)
                .unwrap()
                .discovery_target,
            650
        );
        // Completed legacy imports keep their existing compatibility boundary.
        settings::update_settings_file(&directory, |mut saved| {
            saved.cubes[0].bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Ready {
                wallet: wallet.clone(),
            };
            Some(saved)
        })
        .await
        .unwrap();
        journal.restored_discovery_target = None;
        journal.phase = Phase::Attached;
        journal.persist(&directory).unwrap();
        write_json(&data.path().join(OWNER_FILE), &journal).unwrap();
        assert!(restore_from_kit(
            &directory,
            &blob.cube.uuid,
            &blob,
            backend(),
            RestoredKitSource::Password,
        )
        .await
        .unwrap()
        .bitcoin_import
        .unwrap()
        .is_ready());
    }

    #[tokio::test]
    async fn encrypted_source_and_target_restore_preserve_operation_and_validate_pair() {
        let (source, target) = kits();
        for legacy_source in [false, true] {
            let root = RestoreRoot::new();
            let mut source = source.clone();
            if legacy_source {
                source.import_operation_id = None;
            }
            let source = encrypted_roundtrip(&source);
            let target = encrypted_roundtrip(&target);
            seed_only_cube(&root.0, &source).await;
            seed_only_cube(&root.0, &target).await;
            let mut restored_cubes = Vec::new();
            for blob in [&source, &target] {
                let directory = root.0.network_directory(blob.wallet.descriptor.chain());
                let restored = restore_from_kit(
                    &directory,
                    &blob.cube.uuid,
                    blob,
                    backend(),
                    RestoredKitSource::Password,
                )
                .await
                .unwrap();
                let import = restored.bitcoin_import.as_ref().unwrap();
                if let Some(operation_id) = blob.import_operation_id {
                    assert_eq!(import.operation_id, operation_id);
                }
                let wallet = import.active_attachment().unwrap();
                assert_eq!(wallet.descriptor, blob.wallet.descriptor);
                let saved = Settings::from_file(&directory).unwrap();
                let metadata = saved
                    .wallets
                    .iter()
                    .find(|w| w.wallet_id() == wallet.wallet_id)
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(metadata).unwrap(),
                    serde_json::to_value(blob.signer_metadata.as_ref().unwrap()).unwrap()
                );
                let data = directory.coincubed_data_directory(&wallet.wallet_id);
                let config = Config::from_file(Some(data.path().join(CONFIG_FILE))).unwrap();
                assert_eq!(
                    coincubed::read_imported_address_state(&config, &data).unwrap(),
                    (blob.wallet.receive_index, blob.wallet.change_index)
                );
                restored_cubes.push(restored);
            }
            assert_eq!(
                source.wallet.descriptor.receive_descriptor(),
                target.wallet.descriptor.receive_descriptor()
            );
            assert_eq!(
                source.wallet.descriptor.change_descriptor(),
                target.wallet.descriptor.change_descriptor()
            );
            if legacy_source {
                // A legacy Kit restores the wallet, but cannot invent the old pair binding.
                assert!(pairing::validate_pair(&root.0, &restored_cubes[1]).is_err());
            } else {
                let verified = pairing::validate_pair(&root.0, &restored_cubes[1]).unwrap();
                assert_eq!(verified.bitcoin_import, restored_cubes[0].bitcoin_import);
                let directory = root.0.network_directory(ChainId::Bitcoin);
                let before = fs::read(directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap();
                let mut conflicting = source.clone();
                conflicting.import_operation_id = Some(Uuid::new_v4());
                let error = restore_from_kit(
                    &directory,
                    &source.cube.uuid,
                    &conflicting,
                    backend(),
                    RestoredKitSource::Password,
                )
                .await
                .unwrap_err();
                assert!(error.contains("another import operation"));
                assert_eq!(
                    fs::read(directory.path().join(settings::SETTINGS_FILE_NAME)).unwrap(),
                    before
                );
                assert!(pairing::validate_pair(&root.0, &restored_cubes[1]).is_ok());
            }
        }
    }
}
