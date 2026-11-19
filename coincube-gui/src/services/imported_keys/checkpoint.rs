//! Live-inventory publication transaction. The anchored manifest is the durable
//! journal; a network callback or cloneable UI value is never local authority.
//! Prepared and Confirmed inventories refuse key mutation and operation removal
//! until an authenticated later server tombstone is durably recorded locally.

use super::*;
use crate::services::{
    coincube::{
        imported_checkpoint::{Checkpoint, CheckpointError},
        CoincubeClient, CubeResponse,
    },
    recovery::{
        imported_key_v3::{ImportedKeyKitV3, KitKey},
        ImportedDescriptorBlob,
    },
};

const MAX_CIPHERTEXT: usize = 128 * 1024;

/// Only opaque encrypted bytes, never plaintext key material. Internal clones
/// carry identical bytes across an unlocked network await, not signing grants.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub(super) version: u8,
    pub(super) transaction_id: Uuid,
    pub(super) cube_id: u64,
    pub(super) source_generation: u64,
    pub(super) source_digest: String,
    pub(super) public_digest: String,
    pub(super) generation: u64,
    pub(super) commitment: String,
    /// Some means Prepared; None means Confirmed. Never regenerate after intent.
    pub(super) ciphertext: Option<String>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedCheckpointJournal")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PublicationError {
    Transport(CheckpointError),
    Inventory,
    PublicState,
    Scope,
    Cancelled,
    Bound,
    Stale,
}

impl From<CheckpointError> for PublicationError {
    fn from(value: CheckpointError) -> Self {
        Self::Transport(value)
    }
}

/// A durable backup receipt only. It never admits a key or authorizes signing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocalCheckpointBinding {
    pub cube_id: u64,
    pub source_generation: u64,
    pub generation: u64,
    pub commitment: String,
}

fn hash(ciphertext: &str) -> String {
    hex::encode(sha256::Hash::hash(ciphertext.as_bytes()).to_byte_array())
}

fn public_digest(public: &ImportedDescriptorBlob) -> Result<String, PublicationError> {
    crate::services::recovery::imported_descriptor_blob_fingerprint(public)
        .ok_or(PublicationError::PublicState)
}

fn source_digest(
    repository: &LocalKeyRepository<'_>,
    manifest: &Manifest,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&repository.manifest(manifest.records.clone()))
        .map_err(|_| "Cannot encode imported inventory identity.")?;
    Ok(hex::encode(anchor::digest(&bytes)))
}

pub(super) fn ensure_unbound(manifest: &Manifest) -> Result<(), String> {
    if manifest.checkpoint.is_some() || manifest.restored.is_some() {
        return Err("Imported inventory has a remote Recovery Kit; reconcile or revoke its checkpoint before changing keys.".into());
    }
    Ok(())
}

pub(super) fn validate_journal(
    repository: &LocalKeyRepository<'_>,
    manifest: &Manifest,
) -> Result<(), String> {
    if let Some(journal) = &manifest.checkpoint {
        let shape = Checkpoint {
            generation: journal.generation,
            commitment: journal.commitment.clone(),
            active: true,
        };
        if journal.version != 1
            || journal.transaction_id.is_nil()
            || journal.cube_id == 0
            || journal.source_generation == 0
            || journal.generation == 0
            || shape.validate().is_err()
            || manifest.records.is_empty()
            || journal.public_digest.len() != 64
            || !journal
                .public_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || journal.source_digest != source_digest(repository, manifest)?
            || journal.ciphertext.as_ref().is_some_and(|bytes| {
                bytes.is_empty()
                    || bytes.len() > MAX_CIPHERTEXT
                    || hash(bytes) != journal.commitment
            })
        {
            return Err("Imported checkpoint journal differs from its inventory.".into());
        }
    }
    Ok(())
}

pub(super) fn validate_generation(manifest: &Manifest, authority: &Anchor) -> Result<(), String> {
    if let Some(journal) = &manifest.checkpoint {
        let offset = if journal.ciphertext.is_some() { 1 } else { 2 };
        if !matches!(&authority.state, AnchorState::Committed { generation, .. }
            if journal.source_generation.checked_add(offset) == Some(*generation))
        {
            return Err("Imported checkpoint journal has a stale inventory epoch.".into());
        }
    }
    Ok(())
}

impl Journal {
    fn matches(&self, remote: &Checkpoint) -> bool {
        remote.validate().is_ok()
            && remote.active
            && remote.generation == self.generation
            && remote.commitment == self.commitment
    }
    fn binding(&self) -> LocalCheckpointBinding {
        LocalCheckpointBinding {
            cube_id: self.cube_id,
            source_generation: self.source_generation,
            generation: self.generation,
            commitment: self.commitment.clone(),
        }
    }
}

// A small private interface keeps the cancellation race policy executable
// against both the real transport and deterministic CAS server simulations.
trait CheckpointRemote {
    async fn status(&self, cube_id: u64) -> Result<Checkpoint, CheckpointError>;
    async fn revoke(&self, cube_id: u64, expected: u64) -> Result<Checkpoint, CheckpointError>;
}

impl CheckpointRemote for CoincubeClient {
    async fn status(&self, cube_id: u64) -> Result<Checkpoint, CheckpointError> {
        self.get_imported_checkpoint(cube_id).await
    }
    async fn revoke(&self, cube_id: u64, expected: u64) -> Result<Checkpoint, CheckpointError> {
        self.delete_imported_checkpoint(cube_id, expected).await
    }
}

async fn fence_checkpoint_attempt(
    api: &impl CheckpointRemote,
    journal: &Journal,
    is_current: impl Fn() -> bool,
) -> Result<Checkpoint, PublicationError> {
    let floor = if journal.ciphertext.is_some() {
        journal.generation - 1
    } else {
        journal.generation
    };
    // DELETE and an in-flight PUT can race once. If PUT wins, revoke only its
    // exact known ciphertext at the new epoch. Never chase another active Kit.
    let mut attempted = None;
    for attempt in 0..=2 {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let remote = api.status(journal.cube_id).await?;
        remote.validate()?;
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let Some(expected) = checkpoint_fence_step(journal, &remote, floor, attempt == 2)? else {
            return Ok(remote);
        };
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        if attempted == Some(expected) {
            return Err(PublicationError::Stale);
        }
        attempted = Some(expected);
        match api.revoke(journal.cube_id, expected).await {
            Ok(_)
            | Err(
                CheckpointError::Conflict
                | CheckpointError::Unavailable
                | CheckpointError::InvalidResponse,
            ) => {}
            Err(error) => return Err(error.into()),
        }
        // A response, including an uncertain one, is reconciled by the next
        // authenticated GET. No successful local release is inferred from it.
    }
    unreachable!("bounded cancellation loop always returns")
}

fn checkpoint_fence_step(
    journal: &Journal,
    remote: &Checkpoint,
    floor: u64,
    last_attempt: bool,
) -> Result<Option<u64>, PublicationError> {
    if !remote.active && remote.generation > floor {
        return Ok(None);
    }
    if last_attempt {
        return Err(PublicationError::Stale);
    }
    let expected = if journal.matches(remote) {
        remote.generation
    } else if journal.ciphertext.is_some() && !remote.active && remote.generation == floor {
        floor
    } else {
        return Err(PublicationError::Stale);
    };
    Ok(Some(expected))
}

impl LocalKeyRepository<'_> {
    pub(crate) fn live_checkpoint_public(
        &self,
    ) -> Result<ImportedDescriptorBlob, PublicationError> {
        let settings =
            Settings::from_file(self.directory).map_err(|_| PublicationError::PublicState)?;
        let mut cubes = settings.cubes.iter().filter(|cube| cube.id == self.cube.id);
        let cube = cubes.next().ok_or(PublicationError::PublicState)?;
        if cubes.next().is_some()
            || cube.network != self.cube.network
            || cube.bitcoin_import.as_ref().is_none_or(|import| {
                import.operation_id != self.operation_id
                    || import.active_attachment() != Some(self.attachment)
            })
        {
            return Err(PublicationError::PublicState);
        }
        let mut wallets = settings
            .wallets
            .iter()
            .filter(|wallet| wallet.wallet_id() == self.attachment.wallet_id);
        let mut metadata = wallets.next().ok_or(PublicationError::PublicState)?.clone();
        if wallets.next().is_some() {
            return Err(PublicationError::PublicState);
        }
        metadata.remote_backend_auth = None;
        metadata.start_internal_bitcoind = Some(false);
        metadata.pending_rescan = None;
        let data = self
            .directory
            .coincubed_data_directory(&self.attachment.wallet_id);
        let config = coincubed::config::Config::from_file(Some(data.path().join("daemon.toml")))
            .map_err(|_| PublicationError::PublicState)?;
        if config.main_descriptor.as_imported() != Some(&self.attachment.descriptor)
            || config.bitcoin_config.chain != self.cube.network
            || config.import_birthday != Some(self.attachment.birthday)
            || config.import_initial_lookahead != Some(self.attachment.initial_lookahead)
        {
            return Err(PublicationError::PublicState);
        }
        let address = coincubed::read_imported_claim_state(&config, &data)
            .map_err(|_| PublicationError::PublicState)?;
        ImportedDescriptorBlob::new_with_pair(
            cube.id.clone(),
            cube.network,
            crate::services::recovery::ImportedDescriptorBlobWallet {
                name: cube.name.clone(),
                descriptor: self.attachment.descriptor.clone(),
                birthday: self.attachment.birthday,
                initial_lookahead: self.attachment.initial_lookahead,
                receive_index: address.receive_index,
                change_index: address.change_index,
            },
            cube.imported_claim_source.clone(),
            Some(metadata),
            Some(self.operation_id),
        )
        .map_err(|_| PublicationError::PublicState)
    }

    fn checkpoint_scope(&self, cube: &CubeResponse) -> Result<(), PublicationError> {
        if cube.id == 0 || cube.uuid != self.cube.id || cube.network != self.cube.network.api_str()
        {
            return Err(PublicationError::Scope);
        }
        Ok(())
    }

    /// Called under the stable operation lock, before any revocation/deletion.
    /// An ambiguous local transaction must first be recovered with its unlock.
    pub(super) fn checkpoint_removal_gate(&self, authority: &Anchor) -> Result<(), String> {
        let AnchorState::Committed { digest, .. } = &authority.state else {
            return Err("Recover the imported inventory before removing this operation.".into());
        };
        let empty = serde_json::to_vec(&self.manifest(vec![]))
            .map_err(|_| "Cannot encode empty imported inventory.")?;
        if anchor::digest(&empty) == *digest {
            return Ok(()); // The device anchor itself proves no bound local keys.
        }
        let manifest = self.read_manifest_structure(&self.root().join(MANIFEST), false)?;
        let bytes =
            serde_json::to_vec(&manifest).map_err(|_| "Cannot encode imported inventory.")?;
        if anchor::digest(&bytes) != *digest {
            return Err("Imported removal inventory differs from its device anchor.".into());
        }
        ensure_unbound(&manifest)
    }

    /// Fetch scope/status without mutation, then authenticate and export the
    /// actual live records under the inventory lock. Call off the UI thread.
    pub(crate) async fn prepare_checkpoint(
        &self,
        client: &CoincubeClient,
        cube_id: u64,
        password: &Zeroizing<String>,
        is_current: impl Fn() -> bool,
    ) -> Result<(), PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let cube = client.get_imported_checkpoint_cube(cube_id).await?;
        let remote = client.get_imported_checkpoint(cube_id).await?;
        let credential = self
            .check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let lock = self.lock().map_err(|_| PublicationError::Inventory)?;
        let manifest = self
            .read_manifest_recoverable(&lock)
            .map_err(|_| PublicationError::Inventory)?;
        let device = self
            .device(false)
            .map_err(|_| PublicationError::Inventory)?;
        let public = self.live_checkpoint_public()?;
        let expected_public_digest = public_digest(&public)?;
        self.prepare_checkpoint_locked(
            manifest,
            &cube,
            &remote,
            public,
            password,
            &credential,
            &device,
            || {
                is_current()
                    && self.check_active().is_ok()
                    && self
                        .live_checkpoint_public()
                        .and_then(|blob| public_digest(&blob))
                        .ok()
                        .as_ref()
                        == Some(&expected_public_digest)
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_checkpoint_locked(
        &self,
        manifest: Manifest,
        cube: &CubeResponse,
        remote: &Checkpoint,
        public: ImportedDescriptorBlob,
        password: &Zeroizing<String>,
        credential: &str,
        device: &coincube_core::seed_crypt::DeviceSecret,
        is_current: impl Fn() -> bool,
    ) -> Result<(), PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        self.checkpoint_scope(cube)?;
        ensure_unbound(&manifest).map_err(|_| PublicationError::Bound)?;
        remote.validate()?;
        // An active remote Kit with no authenticated local binding is unknown
        // state, not permission to replace another device's signer inventory.
        if remote.active {
            return Err(PublicationError::Bound);
        }
        if public.cube.uuid != self.cube.id
            || public.wallet.descriptor != self.attachment.descriptor
            || public.import_operation_id != Some(self.operation_id)
            || public.validate(self.cube.network).is_err()
        {
            return Err(PublicationError::Scope);
        }
        let authority = anchor::load(&self.anchor_scope())
            .map_err(|_| PublicationError::Inventory)?
            .ok_or(PublicationError::Inventory)?;
        let AnchorState::Committed {
            generation,
            digest,
            cleanup_txn: None,
            retire_files,
        } = &authority.state
        else {
            return Err(PublicationError::Inventory);
        };
        if *generation == 0
            || !retire_files.is_empty()
            || manifest.records.is_empty()
            || anchor::digest(
                &serde_json::to_vec(&manifest).map_err(|_| PublicationError::Inventory)?,
            ) != *digest
        {
            return Err(PublicationError::Inventory);
        }
        let remote_generation = remote
            .generation
            .checked_add(1)
            .ok_or(PublicationError::Stale)?;
        generation
            .checked_add(2)
            .ok_or(PublicationError::Inventory)?;
        let mut local_keys = Vec::with_capacity(manifest.records.len());
        for record in &manifest.records {
            let bytes = read_key_frame(&self.root().join(record.file_name()))
                .map_err(|_| PublicationError::Inventory)?;
            if bytes[imported_key_crypt::MARKER.len()] != DEVICE_BOUND_FORMAT_VERSION {
                return Err(PublicationError::Inventory);
            }
            let secret = imported_key_crypt::open(
                &bytes,
                credential,
                &self
                    .binding(record)
                    .map_err(|_| PublicationError::Inventory)?,
                Some(device),
            )
            .map_err(|_| PublicationError::Inventory)?;
            local_keys.push(KitKey {
                record: record.clone(),
                secret,
            });
        }
        let public_digest = public_digest(&public)?;
        let kit = ImportedKeyKitV3 {
            public,
            operation_id: self.operation_id,
            wallet_id: self.attachment.wallet_id.to_string(),
            policy_hash: hex::encode(self.policy_hash),
            inventory_generation: *generation,
            checkpoint_generation: remote_generation,
            local_keys,
        };
        // The v3 validator rederives every exported phrase/account and checks
        // both descriptor branches before the password envelope is produced.
        let ciphertext = kit
            .encrypt_password(password)
            .map_err(|_| PublicationError::Inventory)?;
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let journal = Journal {
            version: 1,
            transaction_id: Uuid::new_v4(),
            cube_id: cube.id,
            source_generation: *generation,
            public_digest,
            source_digest: source_digest(self, &manifest)
                .map_err(|_| PublicationError::Inventory)?,
            generation: remote_generation,
            commitment: hash(&ciphertext),
            ciphertext: Some(ciphertext),
        };
        let mut next = manifest.clone();
        next.checkpoint = Some(journal);
        self.publish_manifest(&manifest, &next, credential, device, &is_current)
            .map_err(|_| {
                if is_current() {
                    PublicationError::Inventory
                } else {
                    PublicationError::Cancelled
                }
            })?;
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        Ok(())
    }

    fn checkpoint_snapshot(&self, require_public: bool) -> Result<Journal, PublicationError> {
        let credential = self
            .check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let lock = self.lock().map_err(|_| PublicationError::Inventory)?;
        let manifest = self
            .read_manifest_recoverable(&lock)
            .map_err(|_| PublicationError::Inventory)?;
        let device = self
            .device(false)
            .map_err(|_| PublicationError::Inventory)?;
        self.verify_records(&manifest, &credential, &device)
            .map_err(|_| PublicationError::Inventory)?;
        self.check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let journal = manifest.checkpoint.ok_or(PublicationError::Inventory)?;
        if require_public
            && public_digest(&self.live_checkpoint_public()?)? != journal.public_digest
        {
            return Err(PublicationError::PublicState);
        }
        Ok(journal)
    }

    /// Reopen the durable immutable attempt. GET first reconciles an earlier
    /// uncertain PUT; an explicit resume sends at most one identical CAS PUT.
    pub(crate) async fn resume_checkpoint(
        &self,
        client: &CoincubeClient,
        is_current: impl Fn() -> bool,
    ) -> Result<LocalCheckpointBinding, PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let journal = self.checkpoint_snapshot(true)?;
        let cube = client.get_imported_checkpoint_cube(journal.cube_id).await?;
        self.checkpoint_scope(&cube)?;
        let mut remote = client.get_imported_checkpoint(journal.cube_id).await?;
        if !journal.matches(&remote) {
            if journal.ciphertext.is_none()
                || remote.active
                || remote.generation != journal.generation - 1
            {
                return Err(PublicationError::Stale);
            }
            if !is_current() {
                return Err(PublicationError::Cancelled);
            }
            if self.checkpoint_snapshot(true)? != journal {
                return Err(PublicationError::Stale);
            }
            if !is_current() {
                return Err(PublicationError::Cancelled);
            }
            self.check_active()
                .map_err(|_| PublicationError::Inventory)?;
            let result = client
                .put_imported_checkpoint(
                    journal.cube_id,
                    remote.generation,
                    journal
                        .ciphertext
                        .as_deref()
                        .ok_or(PublicationError::Inventory)?,
                )
                .await;
            match result {
                Ok(_)
                | Err(
                    CheckpointError::Conflict
                    | CheckpointError::Unavailable
                    | CheckpointError::InvalidResponse,
                ) => {}
                Err(error) => return Err(error.into()),
            }
            // Uncertain writes and 409 are reconciled only by current proof.
            remote = client.get_imported_checkpoint(journal.cube_id).await?;
        }
        let cube = client.get_imported_checkpoint_cube(journal.cube_id).await?;
        let credential = self
            .check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let lock = self.lock().map_err(|_| PublicationError::Inventory)?;
        let manifest = self
            .read_manifest_recoverable(&lock)
            .map_err(|_| PublicationError::Inventory)?;
        let device = self
            .device(false)
            .map_err(|_| PublicationError::Inventory)?;
        self.confirm_checkpoint_locked(
            manifest,
            &journal,
            &cube,
            &remote,
            &credential,
            &device,
            || {
                is_current()
                    && self.check_active().is_ok()
                    && self
                        .live_checkpoint_public()
                        .and_then(|blob| public_digest(&blob))
                        .ok()
                        .as_ref()
                        == Some(&journal.public_digest)
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn confirm_checkpoint_locked(
        &self,
        manifest: Manifest,
        expected: &Journal,
        cube: &CubeResponse,
        remote: &Checkpoint,
        credential: &str,
        device: &coincube_core::seed_crypt::DeviceSecret,
        is_current: impl Fn() -> bool,
    ) -> Result<LocalCheckpointBinding, PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        self.checkpoint_scope(cube)?;
        let journal = manifest
            .checkpoint
            .as_ref()
            .ok_or(PublicationError::Inventory)?;
        if journal != expected || cube.id != journal.cube_id || !journal.matches(remote) {
            return Err(PublicationError::Stale);
        }
        self.verify_records(&manifest, credential, device)
            .map_err(|_| PublicationError::Inventory)?;
        if journal.ciphertext.is_none() {
            return Ok(journal.binding());
        }
        let mut confirmed = journal.clone();
        confirmed.ciphertext = None;
        let binding = confirmed.binding();
        let mut next = manifest.clone();
        next.checkpoint = Some(confirmed);
        self.publish_manifest(&manifest, &next, credential, device, &is_current)
            .map_err(|_| {
                if is_current() {
                    PublicationError::Inventory
                } else {
                    PublicationError::Cancelled
                }
            })?;
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        Ok(binding)
    }

    /// Explicitly fence/cancel this exact publication. Requires API #364 for
    /// initial expected=0 revocation; an older server remains fail-closed.
    pub(crate) async fn cancel_checkpoint(
        &self,
        client: &CoincubeClient,
        is_current: impl Fn() -> bool,
    ) -> Result<(), PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let journal = self.checkpoint_snapshot(false)?;
        let cube = client.get_imported_checkpoint_cube(journal.cube_id).await?;
        self.checkpoint_scope(&cube)?;
        fence_checkpoint_attempt(client, &journal, || {
            is_current() && self.check_active().is_ok()
        })
        .await?;
        let cube = client.get_imported_checkpoint_cube(journal.cube_id).await?;
        let remote = client.get_imported_checkpoint(journal.cube_id).await?;
        let credential = self
            .check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let lock = self.lock().map_err(|_| PublicationError::Inventory)?;
        let manifest = self
            .read_manifest_recoverable(&lock)
            .map_err(|_| PublicationError::Inventory)?;
        let device = self
            .device(false)
            .map_err(|_| PublicationError::Inventory)?;
        self.release_checkpoint_locked(
            manifest,
            &journal,
            &cube,
            &remote,
            &credential,
            &device,
            || is_current() && self.check_active().is_ok(),
        )
    }

    /// No remote mutation is hidden here. A caller must first complete explicit
    /// server revocation. Only a later authenticated inactive epoch fences every
    /// old CAS and permits this crash-safe local release; active epochs never do.
    pub(crate) async fn release_checkpoint_after_revocation(
        &self,
        client: &CoincubeClient,
        is_current: impl Fn() -> bool,
    ) -> Result<(), PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        let journal = self.checkpoint_snapshot(false)?;
        let cube = client.get_imported_checkpoint_cube(journal.cube_id).await?;
        let remote = client.get_imported_checkpoint(journal.cube_id).await?;
        let credential = self
            .check_active()
            .map_err(|_| PublicationError::Inventory)?;
        let lock = self.lock().map_err(|_| PublicationError::Inventory)?;
        let manifest = self
            .read_manifest_recoverable(&lock)
            .map_err(|_| PublicationError::Inventory)?;
        let device = self
            .device(false)
            .map_err(|_| PublicationError::Inventory)?;
        self.release_checkpoint_locked(
            manifest,
            &journal,
            &cube,
            &remote,
            &credential,
            &device,
            || is_current() && self.check_active().is_ok(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn release_checkpoint_locked(
        &self,
        manifest: Manifest,
        expected: &Journal,
        cube: &CubeResponse,
        remote: &Checkpoint,
        credential: &str,
        device: &coincube_core::seed_crypt::DeviceSecret,
        is_current: impl Fn() -> bool,
    ) -> Result<(), PublicationError> {
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        self.checkpoint_scope(cube)?;
        remote.validate()?;
        let journal = manifest
            .checkpoint
            .as_ref()
            .ok_or(PublicationError::Inventory)?;
        let floor = if journal.ciphertext.is_some() {
            journal.generation - 1
        } else {
            journal.generation
        };
        if expected != journal
            || cube.id != journal.cube_id
            || remote.active
            || remote.generation <= floor
        {
            return Err(PublicationError::Stale);
        }
        self.verify_records(&manifest, credential, device)
            .map_err(|_| PublicationError::Inventory)?;
        let mut next = manifest.clone();
        next.checkpoint = None;
        self.publish_manifest(&manifest, &next, credential, device, &is_current)
            .map_err(|_| {
                if is_current() {
                    PublicationError::Inventory
                } else {
                    PublicationError::Cancelled
                }
            })?;
        if !is_current() {
            return Err(PublicationError::Cancelled);
        }
        Ok(())
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct Server {
        state: Mutex<ServerState>,
        candidate: String,
        cancel_after_write: Option<Arc<AtomicBool>>,
    }
    struct ServerState {
        generation: u64,
        commitment: String,
        active: bool,
        put_wins: bool,
        lose_delete_reply: bool,
        old_contract: bool,
        deletes: Vec<u64>,
    }
    impl Server {
        fn new(journal: &Journal, put_wins: bool) -> Self {
            Self {
                state: Mutex::new(ServerState {
                    generation: 0,
                    commitment: String::new(),
                    active: false,
                    put_wins,
                    lose_delete_reply: false,
                    old_contract: false,
                    deletes: vec![],
                }),
                candidate: journal.commitment.clone(),
                cancel_after_write: None,
            }
        }
    }
    impl CheckpointRemote for Server {
        async fn status(&self, _cube_id: u64) -> Result<Checkpoint, CheckpointError> {
            let state = self.state.lock().unwrap();
            Ok(Checkpoint {
                generation: state.generation,
                commitment: state.commitment.clone(),
                active: state.active,
            })
        }
        async fn revoke(
            &self,
            _cube_id: u64,
            expected: u64,
        ) -> Result<Checkpoint, CheckpointError> {
            let mut state = self.state.lock().unwrap();
            state.deletes.push(expected);
            if state.put_wins {
                state.put_wins = false;
                state.generation = 1;
                state.commitment = self.candidate.clone();
                state.active = true;
            }
            if state.generation != expected || (state.old_contract && expected == 0) {
                return Err(CheckpointError::Conflict);
            }
            state.generation += 1;
            state.active = false;
            if let Some(cancel) = &self.cancel_after_write {
                cancel.store(false, Ordering::SeqCst);
            }
            if state.lose_delete_reply {
                state.lose_delete_reply = false;
                return Err(CheckpointError::Unavailable);
            }
            Ok(Checkpoint {
                generation: state.generation,
                commitment: state.commitment.clone(),
                active: false,
            })
        }
    }
    fn journal() -> Journal {
        Journal {
            version: 1,
            transaction_id: Uuid::new_v4(),
            cube_id: 42,
            source_generation: 1,
            source_digest: "a".repeat(64),
            public_digest: "b".repeat(64),
            generation: 1,
            commitment: hash("immutable ciphertext"),
            ciphertext: Some("immutable ciphertext".into()),
        }
    }

    #[tokio::test]
    async fn checkpoint_cancel_fences_both_first_put_delete_race_orders() {
        let journal = journal();
        for put_wins in [false, true] {
            let server = Server::new(&journal, put_wins);
            let proof = fence_checkpoint_attempt(&server, &journal, || true)
                .await
                .unwrap();
            assert!(!proof.active);
            let state = server.state.lock().unwrap();
            if put_wins {
                assert_eq!(proof.generation, 2);
                assert_eq!(proof.commitment, journal.commitment);
                assert_eq!(state.deletes, vec![0, 1]);
            } else {
                assert_eq!(proof.generation, 1);
                assert!(proof.commitment.is_empty());
                assert_eq!(state.deletes, vec![0]);
            }
            assert_ne!(
                state.generation, 0,
                "a delayed expected=0 PUT is now fenced"
            );
        }
    }

    #[tokio::test]
    async fn checkpoint_cancel_reconciles_uncertain_delete_without_another_write() {
        let journal = journal();
        let server = Server::new(&journal, false);
        server.state.lock().unwrap().lose_delete_reply = true;
        let proof = fence_checkpoint_attempt(&server, &journal, || true)
            .await
            .unwrap();
        assert_eq!(proof.generation, 1);
        assert_eq!(server.state.lock().unwrap().deletes, vec![0]);
    }

    #[tokio::test]
    async fn checkpoint_cancel_keeps_pending_on_cancel_then_reconciles_exact_tombstone() {
        let journal = journal();
        let current = Arc::new(AtomicBool::new(true));
        let mut server = Server::new(&journal, false);
        server.cancel_after_write = Some(current.clone());
        assert_eq!(
            fence_checkpoint_attempt(&server, &journal, || current.load(Ordering::SeqCst)).await,
            Err(PublicationError::Cancelled)
        );
        let proof = fence_checkpoint_attempt(&server, &journal, || true)
            .await
            .unwrap();
        assert_eq!(proof.generation, 1);
        assert_eq!(server.state.lock().unwrap().deletes, vec![0]);
    }

    #[tokio::test]
    async fn checkpoint_cancel_never_revokes_a_conflicting_active_kit_or_guesses_old_api_success() {
        let journal = journal();
        let server = Server::new(&journal, false);
        {
            let mut state = server.state.lock().unwrap();
            state.generation = 1;
            state.commitment = "c".repeat(64);
            state.active = true;
        }
        assert_eq!(
            fence_checkpoint_attempt(&server, &journal, || true).await,
            Err(PublicationError::Stale)
        );
        assert!(server.state.lock().unwrap().deletes.is_empty());
        let legacy = Server::new(&journal, false);
        legacy.state.lock().unwrap().old_contract = true;
        assert_eq!(
            fence_checkpoint_attempt(&legacy, &journal, || true).await,
            Err(PublicationError::Stale)
        );
        assert_eq!(legacy.state.lock().unwrap().deletes, vec![0]);
    }
}
