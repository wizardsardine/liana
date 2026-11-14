//! Settings is the module to handle the GUI settings file.
//! The settings file is used by the GUI to store useful information.
pub mod bitcoin_import;
pub use bitcoin_import::{BitcoinImport, BitcoinImportStage, ImportedWalletAttachment};
pub mod display;
pub mod fiat;
pub mod preferences;
pub mod system_accessibility;
pub mod unit;

use std::collections::{HashMap, HashSet};

use async_fd_lock::LockWrite;
use coincube_core::descriptors::CoincubeDescriptor;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_ui::component::form;
use serde::{Deserialize, Serialize};

use crate::{
    backup::{Key, KeyRole, KeyType},
    chain::{ChainId, ChainIdExt},
    dir::NetworkDirectory,
    hw::HardwareWalletConfig,
    utils::serde::ok_or_none,
};

pub const SETTINGS_FILE_NAME: &str = "settings.json";
/// Longest wallet alias accepted by the alias editors.
pub const WALLET_ALIAS_MAXIMUM_LENGTH: usize = 64;

/// A settings endpoint refused by path policy rather than by the OS.
fn unsafe_endpoint(reason: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, reason.to_string())
}

/// Inspect a settings endpoint without following a final symbolic link.
/// `Ok(None)` is a genuinely absent endpoint; links and non-regular entries are
/// refused rather than followed.
fn inspect_regular_file(path: &std::path::Path) -> std::io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(unsafe_endpoint("is a symbolic link"))
        }
        Ok(metadata) if !metadata.is_file() => Err(unsafe_endpoint("is not a regular file")),
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Confirm an opened handle is the regular file currently at `path`. On Unix
/// the device/inode pair also detects a swap between inspection and open;
/// elsewhere only the entry types are rechecked. Neither is a guarantee against
/// a hostile same-user process rewriting links.
fn verify_opened_file(file: &std::fs::File, path: &std::path::Path) -> std::io::Result<()> {
    let opened = file.metadata()?;
    let current =
        inspect_regular_file(path)?.ok_or_else(|| unsafe_endpoint("disappeared while opening"))?;
    if !opened.is_file() {
        return Err(unsafe_endpoint("is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened.dev(), opened.ino()) != (current.dev(), current.ino()) {
            return Err(unsafe_endpoint("changed while opening"));
        }
    }
    #[cfg(not(unix))]
    let _ = current;
    Ok(())
}

/// Open an existing coordination lock file. `Ok(None)` when it has never been
/// created; its presence alone never means anything is locked.
fn open_existing_lock(path: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    if inspect_regular_file(path)?.is_none() {
        return Ok(None);
    }
    let file = std::fs::File::open(path)?;
    verify_opened_file(&file, path)?;
    Ok(Some(file))
}

/// Open a coordination lock file, creating it owner-only if absent. A lock
/// file is never unlinked: a waiter must not lock an obsolete inode.
fn open_or_create_lock(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(file) = open_existing_lock(path)? {
        return Ok(file);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => {
            verify_opened_file(&file, path)?;
            Ok(file)
        }
        // Another cooperating process created it first; use that inode.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            open_existing_lock(path)?.ok_or_else(|| unsafe_endpoint("disappeared while opening"))
        }
        Err(error) => Err(error),
    }
}

/// Rejects a JSON document that repeats an object key at any depth. A
/// `serde_json::Value` silently keeps only the last duplicate, so rewriting such
/// a document could change which value an older or newer reader sees.
struct UniqueKeys;

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeys)
    }
}

impl<'de> serde::de::Visitor<'de> for UniqueKeys {
    type Value = UniqueKeys;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Self, E> {
        Ok(UniqueKeys)
    }
    fn visit_some<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Self, D::Error> {
        UniqueKeys::deserialize(deserializer)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self, A::Error> {
        while seq.next_element::<UniqueKeys>()?.is_some() {}
        Ok(UniqueKeys)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self, A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(serde::de::Error::custom("duplicate object key"));
            }
            map.next_value::<UniqueKeys>()?;
        }
        Ok(UniqueKeys)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub cubes: Vec<CubeSettings>,
    #[serde(default)]
    pub wallets: Vec<WalletSettings>,
    /// Global fiat-native vs. bitcoin-native display preference.
    /// Drives whether wallet headers lead with the fiat or the bitcoin
    /// value across the app. Defaults to fiat-native.
    #[serde(default)]
    pub display_mode: display::DisplayMode,
}

impl Settings {
    pub fn from_file(network_dir: &NetworkDirectory) -> Result<Settings, SettingsError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(SETTINGS_FILE_NAME);

        // Retry the transient sharing/visibility failures Windows raises when a
        // virus scanner or search indexer briefly holds a just-written
        // settings.json (or its directory): std::fs::read can momentarily return
        // ACCESS_DENIED ("os error 5") or PATH/FILE_NOT_FOUND ("os error 2/3")
        // for a file that does exist. It can also return a *successful* but
        // empty or partially-written read for a file whose full contents are
        // already durable on disk (this reader takes no file lock — see
        // `update_settings_file`), which then fails to parse. Retry all of these
        // with a short backoff: since every write persists non-empty, valid JSON
        // (flushed + fsync'd under a write lock), an empty read or a parse error
        // right after a write is a transient artifact, not genuine corruption.
        // A NotFound / empty / parse failure that survives the whole budget is
        // surfaced exactly as before — retrying only adds a little latency to
        // those rare paths, never changes their result.
        const MAX_ATTEMPTS: u32 = 5;
        let mut attempt = 0u32;
        loop {
            let retryable = attempt < MAX_ATTEMPTS;
            match std::fs::read(&path) {
                // Full, parseable read — the common path.
                Ok(bytes) if !bytes.is_empty() => {
                    match serde_json::from_slice::<Settings>(&bytes) {
                        Ok(settings) => return Ok(settings),
                        Err(_) if retryable => {}
                        Err(e) => {
                            return Err(SettingsError::ReadingFile(format!(
                                "Parsing settings file: {}",
                                e
                            )))
                        }
                    }
                }
                // Existing-but-empty read: a just-written file can momentarily
                // read back empty. Retry; if it persists, treat as corrupt.
                Ok(_) if retryable => {}
                Ok(_) => {
                    return Err(SettingsError::ReadingFile(
                        "Reading settings file: file was empty".to_string(),
                    ))
                }
                Err(e)
                    if retryable
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                        ) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(SettingsError::NotFound)
                }
                Err(e) => {
                    return Err(SettingsError::ReadingFile(format!(
                        "Reading settings file: {}",
                        e
                    )))
                }
            }
            attempt += 1;
            std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)));
        }
    }
}

/// Read `settings.json`, hand it to `updater`, and write back what comes out.
///
/// # `None` DELETES the file
///
/// The `Option` is not "skip the write". Returning `None` unlinks
/// `settings.json`, taking **every Cube on this network** with it — that is the
/// deliberate behaviour a caller uses to remove the file, and it is the only
/// thing `None` means here.
///
/// The hazard is that `Option` is also what `?` produces on a failed lookup, so
/// the natural-looking updater
///
/// ```ignore
/// update_settings_file(&dir, |mut settings| {
///     let cube = settings.cubes.iter_mut().find(|c| c.id == id)?; // ← wipes the file
///     cube.some_field = value;
///     Some(settings)
/// })
/// ```
///
/// destroys the user's configuration whenever the lookup misses — a record
/// removed since the caller last looked, or a concurrent rewrite. It reads as a
/// no-op and is the opposite of one.
///
/// An updater that means "leave it alone" must return `Some(settings)`
/// unchanged. The safest shape is a named `Fn(Settings) -> Settings` wrapped at
/// the call site as `|s| Some(f(s, ..))`, which makes the wipe unrepresentable
/// from inside the update; see `crate::app::cleared_pending_rescan` and
/// `crate::gui::tab::marked_kit_backed_up`.
///
/// # Errors
///
/// A pre-existing file that will not parse is an error, never silently replaced
/// with defaults — that would discard the stored Cube configuration just as
/// surely as the `None` path.
pub async fn update_settings_file<F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Option<Settings>,
{
    update_settings_file_checked(network_dir, |settings| Ok(updater(settings))).await
}

/// Fallible mutations preserve each chain's existing writer lock and refuse
/// before serializing or replacing the contents when the updater returns Err.
pub(crate) async fn update_settings_file_checked<F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Result<Option<Settings>, SettingsError>,
{
    // Import installation journals settings and wallet storage separately.
    // Atomic replacement keeps settings readable after a process interruption;
    // all writers must share its stable lock to avoid losing concurrent edits.
    update_fork_settings_checked(network_dir, updater).await
}

/// Settings writers lock a stable sibling inode: replacing the data file must not
/// let a waiter acquire an obsolete inode and overwrite a newer writer.
/// The awaited hook is normally ready; tests pause at the last cancellable point.
#[cfg(test)]
async fn update_fork_settings_file<F, P>(
    network_dir: &NetworkDirectory,
    updater: F,
    before_commit: P,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Option<Settings>,
    P: std::future::Future<Output = ()>,
{
    update_fork_settings_checked_with_hook(
        network_dir,
        |settings| Ok(updater(settings)),
        before_commit,
    )
    .await
}

/// Apply a fallible mutation while holding the same stable writer lock.
/// A conflict returns before creating a temporary replacement file.
pub(crate) async fn update_fork_settings_checked<F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Result<Option<Settings>, SettingsError>,
{
    update_fork_settings_checked_with_hook(network_dir, updater, std::future::ready(())).await
}

async fn update_fork_settings_checked_with_hook<F, P>(
    network_dir: &NetworkDirectory,
    updater: F,
    before_commit: P,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Result<Option<Settings>, SettingsError>,
    P: std::future::Future<Output = ()>,
{
    let dir = network_dir.path();
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    let path = dir.join(SETTINGS_FILE_NAME);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("settings.lock"))
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?
        .lock_write()
        .await
        .map_err(|e| SettingsError::WritingFile(format!("Locking settings: {:?}", e)))?;
    let current = match tokio::fs::read(&path).await {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| SettingsError::ReadingFile(e.to_string()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(e) => return Err(SettingsError::ReadingFile(e.to_string())),
    };
    let Some(updated) = updater(current)? else {
        before_commit.await;
        // No await from the commit through release of the stable writer lock.
        match std::fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(SettingsError::DeletingFile(e.to_string())),
        }
        drop(lock);
        return Ok(());
    };
    let bytes = serde_json::to_vec_pretty(&updated)
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    struct PendingFile(std::path::PathBuf);
    impl Drop for PendingFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let pending = PendingFile(dir.join(format!(".settings-{}.tmp", uuid::Uuid::new_v4())));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&pending.0)
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    file.write_all(&bytes)
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    // Surface an asynchronous write failure before syncing/committing. Tokio's
    // sync_all drains in-flight writes but retains their error for flush.
    file.flush()
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    file.sync_all()
        .await
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    drop(file);
    before_commit.await;
    // Synchronous rename is the commit point. Cancellation cannot release the
    // lock while an asynchronous rename continues in a blocking worker.
    std::fs::rename(&pending.0, &path).map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| SettingsError::WritingFile(e.to_string()))?;
    drop(lock);
    // Preserve the existing permission hardening as well as creating each
    // replacement with owner-only mode above.
    restrict_settings_permissions(&path).await;
    Ok(())
}

/// Owner-only permissions on `settings.json`.
///
/// It no longer holds PIN hashes, but it still describes every Cube on the
/// device — names, ids, fingerprints, Vault association, Connect state. The
/// mnemonics folder has been 0o700/0o400 since forever; this file was left at
/// the process umask, which on a shared box is world-readable.
///
/// Best-effort: a filesystem that can't express the mode (FAT, some network
/// mounts) must not fail a settings write over it. Logged at debug.
async fn restrict_settings_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await
        {
            tracing::debug!("could not restrict permissions on {}: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    {
        // Windows: the datadir sits under the user profile, whose ACL is
        // owner-only by default. Read-only is deliberately NOT set here — this
        // file is rewritten on every settings change.
        let _ = path;
    }
}

/// Metadata for a passkey-derived master key (stored in CubeSettings).
///
/// All fields are non-secret: the credential_id is a public identifier and the
/// rp_id is the relying party domain. The actual PRF output (secret) is never stored.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PasskeyMetadata {
    /// Base64-encoded WebAuthn credential ID
    pub credential_id: String,
    /// Relying Party ID used during registration (e.g., "coincube.io")
    pub rp_id: String,
    /// Unix timestamp when the passkey was registered
    pub created_at: i64,
    /// Human-readable label (e.g., "MacBook iCloud Keychain")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Mark a cube as synced with the remote Connect API.
///
/// `owner` is the account the sync ran under. It is recorded as the Cube's
/// [`CubeSettings::connect_owner`] only on the *first* sync of a Cube that has
/// no owner yet — a later sync (or a Cube already marked synced before owners
/// were tracked) must not attribute it to whoever happens to be signed in.
pub async fn mark_cube_synced(
    network_dir: &NetworkDirectory,
    cube_id: &str,
    owner: Option<ConnectOwner>,
) -> Result<(), SettingsError> {
    update_settings_file(network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
            if !cube.remote_synced && cube.connect_owner.is_none() {
                cube.connect_owner = owner;
            }
            cube.remote_synced = true;
        }
        Some(settings)
    })
    .await
}

/// Backfill helper for Cubes created before
/// [`CubeSettings::master_signer_fingerprint`] was tracked: walks
/// `<datadir>/<network>/mnemonics/` for master-seed files
/// (`mnemonic-<fp>-master_<ts>-<ts>.txt`) and returns the
/// fingerprint of the file whose timestamp matches
/// `cube_created_at` (within [`MASTER_SEED_CREATION_WINDOW_SECS`])
/// and whose contents successfully decrypt with `pin`. Returns
/// `None` when no master seed for this Cube lives on this device
/// (e.g. Cube was created elsewhere and never restored here) or
/// when no file falls inside the creation window.
///
/// PIN check alone is *not* sufficient evidence of ownership —
/// two Cubes can share a PIN, and if this Cube's master file is
/// missing/corrupted the PIN would still decrypt some *other*
/// Cube's seed and we'd silently bind the wrong wallet. The
/// timestamp-window guard is what makes the match safe: the file
/// is written `Utc::now()` milliseconds before `CubeSettings.
/// created_at` is stamped (see `home.rs`), so a tight window
/// uniquely associates the file with this Cube. PIN decryption
/// stays as a second layer.
///
/// Caller is responsible for persisting the result via
/// [`update_settings_file`] and updating their in-memory
/// [`CubeSettings`] copy. Without persistence the next launch
/// would re-run the same scan, so callers should always write
/// the result back.
pub fn derive_master_signer_fingerprint(
    datadir_root: &std::path::Path,
    chain: impl Into<crate::chain::ChainId>,
    pin: &str,
    cube_id: &str,
    cube_created_at: i64,
) -> Option<Fingerprint> {
    let chain = chain.into();
    use coincube_core::signer::{
        MasterSigner, MnemonicFileName, MASTER_SEED_LABEL, MNEMONICS_FOLDER_NAME,
    };
    use std::str::FromStr;

    /// Tolerance (seconds) between a master-seed file's timestamp
    /// and the owning Cube's `created_at`. Two seconds covers the
    /// Argon2 + AES-GCM encryption pause between the two
    /// `Utc::now()` reads in the create-cube path; any wider and
    /// we'd risk admitting a different Cube's file.
    const MASTER_SEED_CREATION_WINDOW_SECS: i64 = 2;

    let mnemonics_folder = datadir_root
        .join(chain.dir_name())
        .join(MNEMONICS_FOLDER_NAME);
    let entries = std::fs::read_dir(&mnemonics_folder).ok()?;

    let mut candidates: Vec<(Fingerprint, i64)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let parsed = MnemonicFileName::from_str(&name).ok()?;
            let (checksum, ts) = parsed.descriptor_info?;
            if !checksum.starts_with(MASTER_SEED_LABEL) {
                return None;
            }
            // Hard ownership filter: only consider files whose
            // creation timestamp falls in the Cube's creation
            // window. A PIN match alone is not enough — see fn
            // doc above.
            if (ts - cube_created_at).abs() > MASTER_SEED_CREATION_WINDOW_SECS {
                return None;
            }
            Some((parsed.fingerprint, ts))
        })
        .collect();
    // If two files happen to fall inside the window (extremely
    // unlikely — would require two Cubes minted within 2s), prefer
    // the closer one so the deterministic order matches user
    // expectation.
    candidates.sort_by_key(|(_, ts)| (ts - cube_created_at).abs());

    candidates.into_iter().map(|(fp, _)| fp).find(|&fp| {
        MasterSigner::from_datadir_by_fingerprint_for_chain(
            datadir_root,
            chain,
            fp,
            Some(pin),
            cube_id,
        )
        .is_ok()
    })
}

/// Outcome of [`derive_connect_encryption_pubkey`].
///
/// The two failure cases are deliberately separate. Collapsing them into
/// `None` — which an earlier revision did — makes a Cube that *should* be
/// registrable disappear silently: it never publishes an encryption pubkey, so
/// Contacts cannot enrol enveloped keys against it and it sits forever in the
/// A5 migration's registration-coverage report as a straggler nobody can
/// explain. One of these is normal; the other is a bug that must be loud.
#[derive(Debug)]
pub enum ConnectEncryptionKey {
    /// The 33-byte compressed public key, lowercase hex.
    Derived(String),
    /// **Expected.** This Cube has no master seed on this device — a
    /// descriptor-only / watch-only restore, or a passkey Cube. There is
    /// nothing to derive from and never will be until a seed arrives, so
    /// callers skip quietly. Connect blinding simply degrades to "this Cube
    /// can't receive enveloped keys".
    NoSeed,
    /// **Unexpected.** A master seed file exists and the Cube just unlocked,
    /// but the seed would not open for us. In practice that means the
    /// credentials we passed disagree with the ones the unlock path used —
    /// most likely the `seed_crypt` `cube_id` AAD binding, or a v3 file
    /// reached without the device secret. Callers must log this loudly and
    /// surface it; skipping quietly is what produces the phantom straggler.
    Unreadable(String),
}

/// Derives this Cube's Connect-blinding encryption **public** key from its
/// master seed (`SPEC-cube-xpub-envelope-v1` §3), returning the 33-byte
/// compressed key as lowercase hex.
///
/// Only the public half is returned: the private scalar lives inside the
/// returned-and-dropped `CubeEncryptionKey` and is zeroized here. Callers
/// persist the hex into [`CubeSettings::connect_encryption_pubkey`] so the
/// later, PIN-less registration wave can publish it (`PLAN-connect-blinding`
/// PR D2) — the *private* half is re-derived on demand at decrypt time and is
/// never stored.
///
/// ## Where the signer comes from
///
/// The session cache first, exactly as the Liquid and Spark loaders do.
/// Verifying the PIN *is* a full Argon2id pass over the seed file, so the
/// unlock that just happened is already holding the decrypted signer; re-reading
/// the file would pay ~831 ms for a result we have. It is also the **only** way
/// this works on a v3 Cube: a v3 seed needs the OS-keystore device secret,
/// `coincube-core` has no keystore access, and so
/// `MasterSigner::from_datadir_by_fingerprint` returns `DeviceSecretRequired`
/// for every v3 file. Reading from disk is the fallback for the entry points
/// that have no session (v1/v2 only).
pub fn derive_connect_encryption_pubkey(
    datadir_root: &std::path::Path,
    chain: impl Into<crate::chain::ChainId>,
    fingerprint: Fingerprint,
    pin: &str,
    cube_id: &str,
) -> ConnectEncryptionKey {
    let chain = chain.into();
    use crate::services::connect::crypto::CubeEncryptionKey;
    use coincube_core::signer::{MasterSigner, SignerError};

    // Legacy sessions carry no ChainId. Forks authenticate only their own
    // chain-specific seed, including the OS-keystore secret for v3 files.
    let cached = if chain.is_blake2b() {
        None
    } else {
        crate::app::session::unlocked_signer(cube_id, fingerprint)
    };
    let loaded = match cached {
        Some(signer) => Ok(signer),
        None if chain.is_blake2b() => crate::services::unlock::open_seed_by_fingerprint(
            datadir_root,
            chain,
            fingerprint,
            pin,
            cube_id,
        ),
        None => MasterSigner::from_datadir_by_fingerprint_for_chain(
            datadir_root,
            chain,
            fingerprint,
            Some(pin),
            cube_id,
        ),
    };

    match loaded {
        Ok(signer) => ConnectEncryptionKey::Derived(
            CubeEncryptionKey::derive(&signer, chain.bitcoin_network()).public_key_hex(),
        ),
        // No seed file for this fingerprint: nothing to derive, and nothing
        // wrong. Mapped the same way `load_breez_client` maps an absent signer.
        Err(SignerError::SignerNotFound(_)) => ConnectEncryptionKey::NoSeed,
        Err(SignerError::MnemonicStorage(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
            ConnectEncryptionKey::NoSeed
        }
        // Everything else means the file is there and we could not open it.
        Err(e) => ConnectEncryptionKey::Unreadable(e.to_string()),
    }
}

/// A Cube's relationship to Connect, rendered as a single tri-state cube icon
/// on the Cubes list (Phase 1 of duress mode). The progression
/// Sovereign → Registered → Backed up mirrors increasing recoverability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CubeConnectState {
    /// Not registered with Connect — local-only, no server-side recovery.
    Sovereign,
    /// Registered with Connect but no Cube Recovery Kit uploaded yet.
    Registered,
    /// Registered with Connect and a Cube Recovery Kit is present.
    BackedUp,
}

/// Cubes represent user accounts that can contain multiple features (Vault, Liquid wallet, etc.)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CubeSettings {
    /// Historical Claim completion height on the fork. Never spending or replay authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_completed_at_height: Option<u64>,
    /// Which Claim sweep the historical completion marker describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_completion_txid: Option<coincube_core::miniscript::bitcoin::Txid>,
    /// Foreign-wallet splits (#568 B5) completed into this Cube's Vault by
    /// earlier builds, one record per completed step 2: the digest of the
    /// foreign source's descriptors, the BTCB2 height of step 2's confirmation
    /// and step 2's txid. Never a descriptor, a key, spending or replay
    /// authority. The foreign Split flow that wrote it is removed; the field
    /// is kept so existing `settings.json` files round-trip unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split_from: Vec<SplitFromRecord>,
    pub id: String,
    pub name: String,
    /// The Spark wallet's nickname, shown as `Spark · ‹nickname›` in the side
    /// panel. Spark has no wallet settings of its own, so its name lives on
    /// the Cube; a Vault's name is its [`WalletSettings::alias`]. `None` (or
    /// blank) shows plain "Spark". Files written before the field existed
    /// load unchanged, and an unset name writes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spark_alias: Option<String>,
    /// The chain this Cube lives on — its *identity*, which decides the
    /// directory it is stored under, the Connect network string, the node
    /// and provider it may talk to, and whether this build can run it at all
    /// ([`ChainId::runtime_support`]). Project through
    /// [`ChainId::bitcoin_network`] only for encoding work.
    ///
    /// Serialised as the same strings `bitcoin::Network` used to write
    /// (`"bitcoin"`, `"testnet4"`, …), so every existing `settings.json`
    /// parses unchanged; a Bitcoin Blake2b Cube writes `"bitcoin-blake2b"`,
    /// which older builds refuse rather than misread as mainnet.
    pub network: ChainId,
    #[serde(default)]
    pub backed_up: bool,
    #[serde(default)]
    pub mfa_done: bool,
    /// Whether this cube has been registered with the Connect API.
    /// Defaults to `false` so that existing cubes are picked up by catch-up sync.
    #[serde(default)]
    pub remote_synced: bool,
    pub created_at: i64,
    /// The Vault wallet for this Cube (optional - may not be set up yet)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_wallet_id: Option<WalletId>,
    /// Imported Bitcoin wallet setup/attachment. Never implies inheritance or Connect hasVault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitcoin_import: Option<Box<BitcoinImport>>,
    /// Exact public source of a dedicated imported-wallet fork claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_claim_source: Option<Box<bitcoin_import::ImportedClaimPair>>,
    /// The vault's descriptor fingerprint
    /// ([`crate::app::wallet::descriptor_id_fingerprint`]), 8 lowercase hex.
    /// The user-facing vault identity — the same value shown in vault
    /// settings, sent in the pairing QR as `wfp`, and displayed by Keychain.
    ///
    /// Persisted because the home cube list has no descriptor in scope: it
    /// holds only [`Self::vault_wallet_id`], and the descriptor lives in the
    /// wallet's `daemon.toml` (absent entirely for remote-backend wallets).
    /// `None` on cubes created before this field existed; those converge the
    /// first time the Cube is opened, via
    /// [`crate::app::App::vault_fingerprint_backfill_task`].
    ///
    /// Always written and cleared together with `vault_wallet_id` — see
    /// [`VaultIdentity`], which is what the assignment sites pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_fingerprint: Option<String>,
    // `security_pin_hash` and `duress_pin_hash` used to live here. Both are
    // gone.
    //
    // They were Argon2id PHC strings at m=19456 KiB, t=2, p=1 — ~27 ms a
    // guess — sitting in the same directory as a seed file that costs ~831 ms
    // a guess. An attacker holding the datadir simply attacked the cheap one:
    // a 4-digit PIN fell in about 4 seconds on 64 cores. Verifying against a
    // secret is now done by *decrypting the thing the secret protects*, so
    // there is exactly one cost and it is the highest one available
    // (PLAN-cube-unlock-hardening, invariant I1). See
    // `crate::services::unlock`.
    //
    // Nothing reads either field any more. `CubeSettings` does not set
    // `deny_unknown_fields`, so a `settings.json` written by an older build
    // still deserializes — the values are simply ignored, and dropped on the
    // next settings write. That is the whole migration.
    /// Fingerprint of this Cube's master seed MasterSigner.
    /// Cube-owned wallets (Vault, Liquid, Spark) derive from this seed.
    /// Imported Bitcoin wallets retain their original external signers.
    /// The serde aliases keep existing settings.json files readable without migration.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "liquid_wallet_signer_fingerprint",
        alias = "breez_wallet_signer_fingerprint"
    )]
    pub master_signer_fingerprint: Option<Fingerprint>,
    /// This Cube's Connect-blinding encryption **public** key — 33-byte
    /// compressed secp256k1, lowercase hex (`SPEC-cube-xpub-envelope-v1` §3).
    ///
    /// Cached here for the same ordering reason as [`Self::liquid_granted`],
    /// mirrored: the private half is derived from the master seed and so is
    /// only computable while the Cube is unlocked (PIN in hand), but the
    /// *registration* that publishes it to Connect happens later — after
    /// Connect sign-in, on a code path with no PIN. So the public half is
    /// derived once at unlock and persisted here; the registration wave reads
    /// it from settings.
    ///
    /// Public material — safe to store in plaintext, safe to log. The private
    /// scalar is never persisted anywhere. `None` means "not derived yet"
    /// (a legacy Cube before its first unlock on this build); it is re-derived
    /// and re-persisted at the next unlock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_encryption_pubkey: Option<String>,
    /// Bitcoin display unit preference for this cube
    #[serde(default)]
    pub unit_setting: unit::UnitSetting,
    /// Fiat price display preference for this cube
    #[serde(default, deserialize_with = "ok_or_none")]
    pub fiat_price: Option<fiat::PriceSetting>,
    /// Persisted pending Liquid -> Vault transfer, used to restore UX state across app restarts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_liquid_to_vault_transfer: Option<PendingLiquidToVaultTransfer>,
    /// The Spark cross-network receive (SideShift → bitcoin) still in
    /// progress, so its page survives a lock or restart. Without it, the Shift
    /// ID (which a refund or SideShift support needs) was gone the moment the
    /// app closed, although the bitcoin itself still arrives.
    ///
    /// Written when the shift is created; cleared when its bitcoin arrives,
    /// when it expires unfunded, and when the user closes its finished page.
    /// Holds no keys: a shift id, the two addresses and the chosen asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_spark_shift: Option<PendingSparkShift>,
    /// Last-seen value of the account's `liquidEnabled` grant (Liquid sunset).
    ///
    /// A cache of server state, persisted because of an ordering problem: the
    /// cube (and with it the Liquid SDK) opens at PIN entry, but the grant is
    /// only known after Connect signs in — later, and not at all on a
    /// local-daemon install. Without a persisted copy, an ungranted-looking
    /// fresh wallet gets its scratch state discarded before the grant ever
    /// arrives, and a granted user would never get a Liquid wallet.
    ///
    /// So: the grant is written here whenever `/connect/features` reports it,
    /// and read back at the *next* cube open to decide whether to keep a fresh
    /// Liquid wallet. The practical consequence is that a newly-granted user
    /// gets their Liquid wallet on the next launch, not mid-session.
    ///
    /// Never consulted to *hide* an existing wallet — see
    /// [`crate::app::features::LiquidGate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquid_granted: Option<bool>,
    /// Passkey metadata for passkey-derived master keys (None for random-generated keys)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey_metadata: Option<PasskeyMetadata>,
    /// When true, the Border Wallet wizard uses a random GridRecoveryPhrase instead
    /// of deriving it from the master seed via BIP-85. Defaults to false (use derived).
    #[serde(default)]
    pub allow_random_grid_phrase: bool,
    /// Privacy toggle: when true, the Home eye-icon has hidden balances on
    /// both the Total Balance block and per-wallet cards. Persists across
    /// sessions so users don't have to re-hide on every launch.
    #[serde(default)]
    pub balance_masked: bool,
    /// SHA-256 fingerprint (hex) of the `DescriptorBlob` plaintext that
    /// was last successfully pushed to this Cube's Connect Recovery Kit.
    /// `None` when no descriptor has ever been backed up. Used by W12 to
    /// detect drift: if the live vault's descriptor/signers now hash to
    /// a different value, the Settings card shows "descriptor changed
    /// since last backup — update now." Cleared on vault deletion and
    /// on `delete_recovery_kit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_kit_last_backed_up_descriptor_fingerprint: Option<String>,
    /// Keychain (owner-self "phone") analogue of
    /// `recovery_kit_last_backed_up_descriptor_fingerprint`: SHA-256 (hex) of
    /// the `DescriptorBlob` plaintext last **sealed to the phone** recovery
    /// envelope. Drives the phone copy's independent drift verdict (per-method
    /// drift, PR 3). `None` when no descriptor was ever phone-sealed; written on
    /// a successful `PhoneSealResult`, cleared alongside the password slot on
    /// Remove-all. Back-compat is free — only the phone-seal path ever writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_kit_last_backed_up_keychain_descriptor_fingerprint: Option<String>,
    /// Whether a **password** Cube Recovery Kit has been pushed to Connect from
    /// this device, independent of whether that kit carried a Wallet Descriptor.
    ///
    /// Exists because the two descriptor-fingerprint slots above are *drift*
    /// signals, not *presence* signals, and a Cube with no Vault has no
    /// descriptor to fingerprint. Keying kit-presence off them alone made a
    /// seed-only (Cube-only) kit read as "no recovery kit" on the Home card
    /// while the Settings card — which asks the server — read "backed up".
    ///
    /// Written on a successful kit upload, cleared on Remove (and on the
    /// partial-remove path where the password kit was already torn down), and
    /// reconciled against `/recovery-kit/status` on every status load — which
    /// is what heals Cubes backed up by a build that predates this field, and
    /// Cubes whose kit was created or removed on another device.
    #[serde(default)]
    pub recovery_kit_password_backed_up: bool,
    /// Whether the one-time "turn on recovery alerts?" consent prompt has been
    /// answered for this Cube's Vault (PLAN-recovery-alerts-cleanup PR 3). Once
    /// `true` — whether the user accepted or declined — the prompt never fires
    /// again: a decline is durable, not nagging. `false`/absent = not yet asked.
    /// The residual nudge (a banner on the Recovery settings card when keyholders
    /// exist and alerts are off) is derived from live state, independent of this.
    #[serde(default)]
    pub recovery_alerts_prompt_answered: bool,
    /// The user's Spark Stable Balance decision for this Cube: `Some(true)`
    /// = keep it on, `Some(false)` = keep it off, `None` = never decided on
    /// this device.
    ///
    /// The Spark SDK keeps the toggle in its own local cache, so it is lost
    /// whenever that cache is — a recovered Cube on a new machine comes up
    /// with Stable Balance off while still holding the USDB an earlier
    /// session swept into it. This copy is the app's own record of what the
    /// user wanted. It is written on every toggle, on a bridge-initiated
    /// pause (as `false`, so the paused loop can't be re-armed at the next
    /// launch), and when the user dismisses the "USDB held but off" banner
    /// (also `false` — a decision, not a snooze). It is reconciled against
    /// the SDK's state after the first sync of each session
    /// ([`crate::app::App`] `reconcile_spark_stable_balance`): the app's
    /// copy wins.
    ///
    /// Also mirrored to the Cube's Connect record (`sparkStableBalance`,
    /// `CubeResponse::spark_stable_balance`) so the owner's other desktops
    /// pick it up: every write here is pushed, the registration request
    /// carries it, and a device with `None` adopts the server's value when
    /// its registration answers. A fresh install without Connect has `None`
    /// and falls back to the wallet's on-chain evidence (a USDB holding) to
    /// prompt the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spark_stable_balance: Option<bool>,
    /// Set when the user finished Cube creation **without** demonstrating a
    /// backup, by explicitly accepting
    /// [`crate::services::unlock::creation_gate::BYPASS_ACKNOWLEDGEMENT`].
    ///
    /// Recorded so support can identify these Cubes from the datadir: since the
    /// seed file is sealed to this machine's keystore, a bypassed Cube whose
    /// machine is lost is unrecoverable, and that has to be answerable without
    /// relying on anyone's recollection. `None` on every Cube that backed up
    /// normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_backup_bypass:
        Option<crate::services::unlock::creation_gate::CreationBackupBypass>,
    /// Set when the user finished Cube creation by **creating a Recovery Kit**
    /// in the creation flow itself.
    ///
    /// The local half of that Kit: the creation gate is evaluated offline at
    /// open time, so a Kit that lives only on Connect cannot satisfy it. `None`
    /// on every Cube that took another exit — a written phrase, the bypass, or
    /// creation before the option existed. See
    /// [`crate::services::unlock::creation_gate::CreationRecoveryKit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_recovery_kit: Option<crate::services::unlock::creation_gate::CreationRecoveryKit>,
    /// Whether this Cube was created under the mandatory-backup gate.
    ///
    /// `false` — the serde default — for every Cube that predates it. That is
    /// deliberate and load-bearing: the gate exists because a v3 seed file
    /// can't be recovered from a copied datadir, and applying it retroactively
    /// would lock every existing user out of a Cube they have been using
    /// happily. Only Cubes created from now on are held to it.
    #[serde(default)]
    pub creation_backup_required: bool,
    /// File name of this Cube's **second slot** in `mnemonics/`.
    ///
    /// Every Cube written since unit 6b has one, from creation. It holds a
    /// duress marker when duress is armed and a decoy when it is not, and the
    /// two are indistinguishable — see `services::unlock::marker`. So the
    /// presence of this field says only "this Cube has a slot", never "duress
    /// is armed on this Cube"; enrolment lives in `DuressLocalState`.
    ///
    /// It is *named* for duress because that is what the slot is for, not
    /// because a populated field implies one. Reading it as an armed flag is
    /// the mistake the decoy exists to make impossible — and a mistake that
    /// would not show up until someone imaged a datadir.
    ///
    /// The name is random and drawn when the slot is first written, so this is
    /// the only way to find the file. Recording it is not a leak: it replaced
    /// a name derived from `id` + `created_at`, which any reader of this file
    /// could simply compute.
    ///
    /// `None` is a Cube written before the field existed, or one whose slot
    /// has yet to be backfilled by migration.
    #[serde(default, alias = "duress_marker_file")]
    pub duress_slot_file: Option<String>,
    /// The Connect account this Cube was first registered under — the
    /// account that owns its server-side record, and so its Vault and the
    /// Keychain signers attached to it.
    ///
    /// Written once, on the Cube's first successful registration (see
    /// [`mark_cube_synced`] and the creation Recovery Kit path), and never
    /// overwritten: signing in as another account re-registers the same local
    /// Cube UUID as a *new* server row, which must not claim ownership. `None`
    /// on Cubes registered before this field existed, and on Cubes never
    /// registered at all — callers treat that as "unknown", not "mismatch".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_owner: Option<ConnectOwner>,
}

/// One Split completed into a Cube's Vault; see [`CubeSettings::split_from`].
/// Digest-only by construction: no field can hold a descriptor or a key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SplitFromRecord {
    /// SHA-256 of the foreign source's canonical descriptors (D9), the
    /// Split journal's identity.
    pub descriptor_digest: coincube_core::miniscript::bitcoin::hashes::sha256::Hash,
    /// BTCB2 height of the block that confirmed step 2 when the completion
    /// was recorded.
    pub completed_height: u64,
    /// The signed step 2's own txid.
    pub step2_txid: coincube_core::miniscript::bitcoin::Txid,
}

/// Identity of the Connect account that owns a Cube. See
/// [`CubeSettings::connect_owner`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConnectOwner {
    /// Connect user id — stable across email changes.
    pub user_id: u32,
    /// Email at registration time, for display ("sign in as …").
    pub email: String,
}

impl ConnectOwner {
    pub fn from_user(user: &crate::services::coincube::User) -> Self {
        Self {
            user_id: user.id,
            email: user.email.clone(),
        }
    }

    /// Whether `email` names this owner. Case-insensitive: Connect emails
    /// are not case-sensitive, and the two copies come from different
    /// responses.
    pub fn is_email(&self, email: &str) -> bool {
        self.email.trim().eq_ignore_ascii_case(email.trim())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PendingLiquidToVaultTransfer {
    pub swap_id: String,
    pub amount_sat: u64,
}

/// A Spark cross-network receive that has a live shift: everything needed to
/// put its page back and resume polling SideShift. See
/// [`CubeSettings::pending_spark_shift`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PendingSparkShift {
    /// The shift as SideShift created it: id, deposit address and limits.
    pub shift: crate::services::sideshift::ShiftResponse,
    /// What the user deposits, as [`crate::services::sideshift::DepositOption::key`].
    pub deposit_option: String,
    /// Where SideShift refunds to, on the deposit's chain.
    pub refund_address: String,
}

impl CubeSettings {
    /// Create a new `CubeSettings` with a caller-supplied id string, taken
    /// verbatim — no round-trip through `uuid::Uuid`, so the byte-exact value
    /// is preserved. Used by Cube recovery, where the restored Cube's id must
    /// string-match the server-side record for the idempotent `register_cube`
    /// call to reactivate the original Cube instead of minting a duplicate
    /// (parsing + re-formatting would normalize case; a malformed value would
    /// force a lossy fallback).
    pub fn new_with_raw_id<C: Into<ChainId>>(id: String, name: String, network: C) -> Self {
        Self {
            split_completed_at_height: None,
            split_completion_txid: None,
            split_from: Vec::new(),
            id,
            name,
            spark_alias: None,
            network: network.into(),
            created_at: chrono::Utc::now().timestamp(),
            vault_wallet_id: None,
            bitcoin_import: None,
            imported_claim_source: None,
            vault_fingerprint: None,
            // Unknown until `/connect/features` answers for this account. A new
            // cube is not granted Liquid; if the account is, the flag lands on
            // the next features fetch and takes effect at the next cube open.
            liquid_granted: None,
            master_signer_fingerprint: None,
            // Derived at the first unlock (the PIN is needed to reach the
            // seed), then persisted — see the field docs.
            connect_encryption_pubkey: None,
            backed_up: false,
            mfa_done: false,
            remote_synced: false,
            unit_setting: unit::UnitSetting::default(),
            fiat_price: Some(fiat::PriceSetting::default()), // Initialize with default (enabled: true)
            pending_liquid_to_vault_transfer: None,
            pending_spark_shift: None,
            passkey_metadata: None,
            allow_random_grid_phrase: false,
            balance_masked: false,
            recovery_kit_last_backed_up_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_keychain_descriptor_fingerprint: None,
            recovery_kit_password_backed_up: false,
            recovery_alerts_prompt_answered: false,
            spark_stable_balance: None,
            creation_backup_bypass: None,
            creation_recovery_kit: None,
            // Set explicitly by the creation flow; `new_*` is also used to
            // reconstruct Cubes in restore paths, which must not be gated.
            creation_backup_required: false,
            duress_slot_file: None,
            connect_owner: None,
        }
    }

    /// Create a new `CubeSettings` with a caller-supplied UUID.
    ///
    /// The frontend should generate this UUID before initiating the creation
    /// request so that retries reuse the same identifier (idempotent creation).
    pub fn new_with_id<C: Into<ChainId>>(id: uuid::Uuid, name: String, network: C) -> Self {
        Self::new_with_raw_id(id.to_string(), name, network)
    }

    pub fn new<C: Into<ChainId>>(name: String, network: C) -> Self {
        Self::new_with_id(uuid::Uuid::new_v4(), name, network)
    }

    pub fn with_vault(mut self, vault: VaultIdentity) -> Self {
        self.set_vault(vault);
        self
    }

    /// Attach a Vault to this Cube: both halves of its identity, together.
    ///
    /// The pairing is the point — see [`VaultIdentity`]. A caller that set the
    /// wallet id and forgot the fingerprint would leave the Cube rendering
    /// "Vault configured" with no id until the next backfill.
    pub fn set_vault(&mut self, vault: VaultIdentity) {
        // Legacy callers cannot overwrite an imported or reserved Bitcoin slot.
        // UI/installer handlers report this conflict before reaching this guard.
        if self.bitcoin_import.is_some() {
            tracing::warn!("Refused to attach a Vault to a Cube with a Bitcoin import");
            return;
        }
        self.vault_wallet_id = Some(vault.wallet_id);
        self.vault_fingerprint = vault.fingerprint;
    }

    /// Adopt `live` — the fingerprint just computed from the loaded wallet's
    /// descriptor — as this Cube's Vault identity, returning whether anything
    /// changed and so whether the settings file needs rewriting.
    ///
    /// This is the D4 backfill's decision, in one place so it can be pinned:
    /// it writes once on a Cube that predates the field, no-ops on every open
    /// after that, and rewrites if the descriptor changed underneath (key
    /// rotation, membership change) — a new descriptor genuinely *is* a new
    /// identity, which is why the human-facing name lives on the Cube.
    pub fn adopt_vault_fingerprint(&mut self, live: &str) -> bool {
        if self.vault_fingerprint.as_deref() == Some(live) {
            return false;
        }
        self.vault_fingerprint = Some(live.to_string());
        true
    }

    /// Detach this Cube's Vault, clearing both halves of its identity.
    ///
    /// Same rationale as the recovery-kit descriptor fingerprint being cleared
    /// on vault deletion: a stale fingerprint outliving the vault it named
    /// would have the home list advertise an id for a Vault that is gone.
    pub fn clear_vault(&mut self) {
        self.vault_wallet_id = None;
        self.vault_fingerprint = None;
    }

    pub fn with_master_signer(mut self, fingerprint: Fingerprint) -> Self {
        self.master_signer_fingerprint = Some(fingerprint);
        self
    }

    pub fn with_passkey(mut self, metadata: PasskeyMetadata) -> Self {
        self.passkey_metadata = Some(metadata);
        self
    }

    /// Whether this Cube uses a passkey-derived master key (no PIN, no stored seed).
    pub fn is_passkey_cube(&self) -> bool {
        self.passkey_metadata.is_some()
    }

    /// True when this Cube has a Cube Recovery Kit pushed to Connect **from this
    /// device**, by either method — password kit or phone (keychain) seal.
    ///
    /// The password kit is answered by its own presence flag
    /// ([`recovery_kit_password_backed_up`](Self::recovery_kit_password_backed_up)),
    /// *not* by its descriptor fingerprint: a Cube with no Vault backs up a
    /// seed-only kit, which has no descriptor to fingerprint, and reading
    /// presence off the fingerprint made those Cubes report "no recovery kit"
    /// while Settings (which asks the server) said "backed up".
    ///
    /// The two descriptor-fingerprint slots still count, for two reasons: the
    /// phone seal always seals a descriptor and has no separate presence flag,
    /// and a Cube backed up by an older build has the password fingerprint but
    /// not the flag — OR-ing keeps it reading "backed up" across the upgrade
    /// with no migration.
    ///
    /// This is a **cached** answer, not a live one: it is written when a backup
    /// is made from this device, and reconciled against the server whenever the
    /// Recovery-Kit status loads. A Cube whose kit was created or removed on
    /// another device reads stale until it is next opened here. Distinct from
    /// [`backed_up`](Self::backed_up), which tracks the *local* seed-phrase
    /// backup, not the server-side kit.
    pub fn has_recovery_kit(&self) -> bool {
        self.recovery_kit_password_backed_up
            || self
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .is_some()
            || self
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .is_some()
    }

    /// Whether the cached Kit record includes the current wallet kind.
    /// A seed-only Kit remains present after import, but is not a complete
    /// imported-wallet backup until an authenticated update records its public
    /// descriptor payload. Live address-state drift is checked separately.
    pub fn has_complete_recovery_kit(&self) -> bool {
        if self.has_imported_bitcoin() {
            self.recovery_kit_last_backed_up_descriptor_fingerprint
                .is_some()
                || self
                    .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                    .is_some()
        } else {
            self.has_recovery_kit()
        }
    }

    /// Classifies this Cube's relationship to Connect for the card indicator
    /// (Phase 1 of duress mode). Users must be able to tell at a glance whether
    /// a Cube has a recovery kit before they're told what a duress wipe costs.
    pub fn connect_state(&self) -> CubeConnectState {
        if !self.remote_synced {
            CubeConnectState::Sovereign
        } else if self.has_complete_recovery_kit() {
            CubeConnectState::BackedUp
        } else {
            CubeConnectState::Registered
        }
    }

    /// Whether this Cube needs a PIN, answered from **ground truth on disk**
    /// rather than from a settings field: a Cube has a PIN if and only if its
    /// master seed file is encrypted.
    ///
    /// This replaces the old `has_pin()`, which read `security_pin_hash
    /// .is_some()` and could drift from reality in either direction — a Cube
    /// could carry a hash with no encrypted seed, or an encrypted seed with no
    /// hash, and nothing would notice.
    ///
    /// There is deliberately **no** `verify_pin` any more. Verifying a PIN
    /// means decrypting the seed file; see
    /// [`crate::services::unlock::unlock_blocking`].
    pub fn pin_requirement(
        &self,
        datadir_root: &std::path::Path,
    ) -> crate::services::unlock::PinRequirement {
        crate::services::unlock::pin_requirement(&crate::services::unlock::CubeLocation::new(
            datadir_root,
            self,
        ))
    }

    /// Whether a PIN is required to open this Cube.
    pub fn has_pin(&self, datadir_root: &std::path::Path) -> bool {
        self.pin_requirement(datadir_root) == crate::services::unlock::PinRequirement::Required
    }

    /// Whether this Cube has its second `mnemonics/` slot on disk.
    ///
    /// Renamed from `has_duress_pin`, which is what it used to mean and no
    /// longer can. Since unit 6b every Cube carries a slot whether or not
    /// duress is enrolled, and a marker is indistinguishable from a decoy by
    /// design — so no filesystem check can answer "is duress armed". That
    /// question is `DuressLocalState::enrolled`.
    ///
    /// What this answers is whether the slot needs backfilling.
    pub fn has_duress_slot(&self, datadir_root: &std::path::Path) -> bool {
        crate::services::unlock::marker::exists(
            datadir_root,
            self.network,
            self.duress_slot_file.as_deref(),
        )
    }

    /// Load Cube settings from file
    pub fn load_from_file(
        network_dir: &crate::dir::NetworkDirectory,
    ) -> Result<Option<Self>, SettingsError> {
        let path = network_dir.path().join("cube_settings.toml");

        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path).map_err(|e| {
            SettingsError::ReadingFile(format!("Failed to read cube settings: {}", e))
        })?;

        let cube_settings: CubeSettings = toml::from_str(&content).map_err(|e| {
            SettingsError::ReadingFile(format!("Failed to parse cube settings: {}", e))
        })?;

        Ok(Some(cube_settings))
    }

    /// Convert this cube's chain to the API network string. Keyed on the
    /// identity: a Bitcoin Blake2b Cube registers as `bitcoin-blake2b`, never
    /// as `mainnet`.
    pub fn api_network_string(&self) -> String {
        self.network.api_str().to_string()
    }

    /// Whether this build can open this Cube. Both Bitcoin Blake2b identities
    /// are dormant in this build: the Cube's settings are kept intact and
    /// distinct, but no daemon, node, SDK or signer may be started for it.
    pub fn runtime_support(&self) -> crate::chain::RuntimeSupport {
        self.network.runtime_support()
    }
}

/// The API network string used by the Connect backend for a chain. Borrowing
/// form — prefer it where an owned `String` isn't needed, such as a
/// comparison inside a predicate the view calls per render. Accepts a
/// `bitcoin::Network` from Bitcoin-family callers (the identity mapping for
/// that family) or a [`ChainId`].
pub fn network_to_api_str<C: Into<ChainId>>(chain: C) -> &'static str {
    chain.into().api_str()
}

/// Owned form of [`network_to_api_str`].
pub fn network_to_api_string<C: Into<ChainId>>(chain: C) -> String {
    network_to_api_str(chain).to_string()
}

/// Liana Connect account a legacy remote-backend Vault was registered under.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub email: String,
    pub wallet_id: String,
    // legacy field, refresh_token is now stored in the connect cache file
    // Keep it in case, user want to open the wallet with a previous Coincube-GUI version.
    // Field cannot be ignored as the settings file is override during settings update.
    // TODO: remove later after multiple versions.
    pub refresh_token: Option<String>,
}

impl AuthConfig {
    pub fn new(email: String, wallet_id: String) -> Self {
        Self {
            email,
            wallet_id,
            refresh_token: None,
        }
    }

    /// Why a legacy Liana Connect Vault cannot be opened here, and where its
    /// wallet still lives. Shown wherever such a Cube is opened or deleted.
    pub fn retired_backend_notice(&self) -> String {
        format!(
            "This Cube's Vault was set up on Liana Connect, which COINCUBE no longer \
             supports. The wallet itself still exists on Liana Connect: open it in the \
             Liana desktop app and sign in as {} to reach your funds. Deleting this Cube \
             removes only the copy on this computer — before you do, make sure you can \
             open the wallet in Liana or have its recovery phrase and descriptor backed up.",
            self.email
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletSettings {
    pub name: String,
    pub alias: Option<String>,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    /// Set on Vaults created on the retired Liana Connect remote backend
    /// (lianalite). COINCUBE no longer talks to that backend: such a Vault
    /// cannot be opened here and is only offered for local deletion, with a
    /// pointer to the Liana app where the wallet still lives.
    pub remote_backend_auth: Option<AuthConfig>,
    /// Start internal bitcoind executable.
    /// if None, the app must refer to the gui.toml start_internal_bitcoind field.
    pub start_internal_bitcoind: Option<bool>,
    /// A rescan this Vault still owes.
    ///
    /// Set when a Vault is restored from a Recovery Kit: the descriptors go into
    /// a bitcoind watchonly wallet that has never seen them, which imports them
    /// at `timestamp: "now"` and so finds none of the wallet's history. The kit
    /// carries the original wallet's birthday (`backup::Account::timestamp`),
    /// which is exactly the point the new node has to scan from.
    ///
    /// Persisted rather than held in memory because the rescan has to survive
    /// the things that interrupt it — the daemon not being up at the moment the
    /// installer exits, a quit part-way through a multi-hour mainnet scan, a
    /// crash. It is cleared only once a rescan has actually been accepted by the
    /// daemon, so an interrupted one is offered again on the next launch.
    ///
    /// `None` on every other flow, including a fresh install, where scanning
    /// from now is correct and there is nothing to catch up on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rescan: Option<PendingRescan>,
    /// Whether [`KeySetting::keychain_key_id`] was recorded for this Vault's
    /// keys, so a key without one is known not to be a Keychain key.
    ///
    /// Set only by a Vault built in the descriptor editor, where every key's
    /// source is known. `false` for every Vault created before the field
    /// existed and for imports and restores, whose keys carry no provenance:
    /// there an absent id means *unrecorded*, not "not a Keychain key".
    /// Absent from the file when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keychain_keys_recorded: bool,
}

/// A rescan a restored Vault owes, and whether we know where to start it.
///
/// Two variants because the two restore sources differ in what they recorded.
/// A backup import carries `Account::timestamp`, the source wallet's birthday,
/// and can start the scan unattended. A **Recovery Kit** carries no birthday at
/// all — `DescriptorBlobVault` is name, descriptor, change descriptor and
/// signers — so the date is genuinely unknown and guessing one would be worse
/// than asking: a scan started too late looks exactly like a completed scan
/// that found nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingRescan {
    /// Scan from this unix time. Started automatically at next open.
    From(u32),
    /// A rescan is owed, but nothing recorded when the wallet was born. The
    /// prompt is raised and the user picks the date in Settings > Node.
    DateUnknown,
}

impl PendingRescan {
    /// The date to scan from, when one is known.
    pub fn timestamp(&self) -> Option<u32> {
        match self {
            Self::From(t) => Some(*t),
            Self::DateUnknown => None,
        }
    }
}

impl WalletSettings {
    pub fn from_file<F>(
        network_dir: &NetworkDirectory,
        selecter: F,
    ) -> Result<Option<Self>, SettingsError>
    where
        F: FnMut(&WalletSettings) -> bool,
    {
        Settings::from_file(network_dir).map(|cache| cache.wallets.into_iter().find(selecter))
    }

    pub fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for key in self.keys.iter().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
        map
    }

    pub fn provider_keys(&self) -> HashMap<Fingerprint, ProviderKey> {
        let mut map = HashMap::new();
        for (fingerprint, provider_key) in self
            .keys
            .iter()
            .filter_map(|k| k.provider_key.as_ref().map(|pk| (k.master_fingerprint, pk)))
        {
            map.insert(fingerprint, provider_key.clone());
        }
        map
    }

    pub fn border_wallet_fingerprints(&self) -> HashSet<Fingerprint> {
        self.keys
            .iter()
            .filter(|k| k.is_border_wallet)
            .map(|k| k.master_fingerprint)
            .collect()
    }

    /// Recorded grid-seed provenance, one entry per Border Wallet key that has
    /// one. A key with no entry has an *unrecorded* provenance — see
    /// [`KeySetting::grid_seed_source`] — which is not the same as
    /// [`GridSeedSource::Independent`], so the map is deliberately sparse
    /// rather than defaulted.
    pub fn border_wallet_grid_seed_sources(&self) -> HashMap<Fingerprint, GridSeedSource> {
        self.keys
            .iter()
            .filter(|k| k.is_border_wallet)
            .filter_map(|k| k.grid_seed_source.map(|src| (k.master_fingerprint, src)))
            .collect()
    }

    /// Signers the user has marked as replay-protected (or explicitly not).
    /// Sparse: an unmarked signer is absent, which the signer index reads as
    /// `UserMarked(false)` for a device and ignores for every other kind.
    pub fn replay_marks(&self) -> HashMap<Fingerprint, bool> {
        self.keys
            .iter()
            .filter_map(|k| k.replay_protected.map(|mark| (k.master_fingerprint, mark)))
            .collect()
    }

    /// Keychain key ids, one entry per key that came from the COINCUBE
    /// Keychain. Sparse, and only complete when
    /// [`Self::keychain_keys_recorded`] is set.
    pub fn keychain_key_ids(&self) -> HashMap<Fingerprint, u64> {
        self.keys
            .iter()
            .filter_map(|k| k.keychain_key_id.map(|id| (k.master_fingerprint, id)))
            .collect()
    }

    pub fn update_alias(&mut self, key: &Fingerprint, alias: &str) {
        let key_aliases = self.keys_aliases();
        if key_aliases.contains_key(key) {
            self.keys = self
                .keys
                .clone()
                .into_iter()
                .map(|mut ks| {
                    if ks.master_fingerprint == *key {
                        ks.name = alias.into();
                        ks
                    } else {
                        ks
                    }
                })
                .collect();
        }
    }

    pub fn wallet_id(&self) -> WalletId {
        WalletId {
            timestamp: self.pinned_at,
            descriptor_checksum: self.descriptor_checksum.clone(),
        }
    }
}

/// Local storage key for a wallet: the descriptor's BIP-380 checksum plus the
/// moment it was pinned.
///
/// **Not a vault identity.** `descriptor_checksum` names things on disk — the
/// wallet's `coincubed` directory, its `mnemonic-…` files (`crate::signer`) —
/// and changing it would strand that data, so it stays as it is. What it must
/// never do again is appear in the UI as the vault's id; that is
/// [`CubeSettings::vault_fingerprint`]
/// (`plans/PLAN-vault-identity-unification.md` D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletId {
    pub timestamp: Option<i64>,
    pub descriptor_checksum: String,
}

/// The two halves of a Cube's Vault association, carried as one value so a
/// caller cannot set the storage key and forget the identity.
///
/// `fingerprint` is `None` only where the descriptor genuinely isn't in scope
/// at attach time (an import path that never saw one). Those Cubes render
/// "Vault configured" until the backfill on first open supplies it — the
/// deliberate choice over showing the checksum, which is what
/// `plans/PLAN-vault-identity-unification.md` exists to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultIdentity {
    /// Storage key — see [`WalletId`].
    pub wallet_id: WalletId,
    /// 8 lowercase hex, from
    /// [`crate::app::wallet::descriptor_id_fingerprint`].
    pub fingerprint: Option<String>,
}

impl VaultIdentity {
    /// Both halves derived from the descriptor that is about to become this
    /// Cube's Vault. `WalletId::generate` stamps `timestamp` with now, so this
    /// is for a *newly pinned* wallet; use the struct literal to re-express an
    /// existing `WalletId`.
    pub fn generate(descriptor: &CoincubeDescriptor) -> Self {
        Self {
            wallet_id: WalletId::generate(descriptor),
            fingerprint: Some(
                crate::app::wallet::descriptor_id_fingerprint(descriptor).to_string(),
            ),
        }
    }

    /// Pair an already-minted [`WalletId`] with the descriptor it came from.
    /// The descriptor is optional because several attach sites are handed a
    /// `WalletId` by an upstream step that may not have kept the descriptor.
    pub fn new(wallet_id: WalletId, descriptor: Option<&CoincubeDescriptor>) -> Self {
        Self {
            wallet_id,
            fingerprint: descriptor
                .map(|d| crate::app::wallet::descriptor_id_fingerprint(d).to_string()),
        }
    }
}

impl WalletId {
    pub fn new(descriptor_checksum: String, timestamp: Option<i64>) -> Self {
        WalletId {
            timestamp,
            descriptor_checksum,
        }
    }
    pub fn generate(descriptor: &CoincubeDescriptor) -> Self {
        WalletId {
            timestamp: Some(chrono::Utc::now().timestamp()),
            descriptor_checksum: descriptor
                .to_string()
                .split_once('#')
                .map(|(_, checksum)| checksum)
                .expect("CoincubeDescriptor.to_string() always include the checksum")
                .to_string(),
        }
    }
    pub fn is_legacy(&self) -> bool {
        self.timestamp.is_none() && !self.descriptor_checksum.starts_with("imported-v1-")
    }
}

impl std::fmt::Display for WalletId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(t) = self.timestamp {
            write!(f, "{}-{}", self.descriptor_checksum, t)
        } else {
            write!(f, "{}", self.descriptor_checksum)
        }
    }
}

/// A key provider recorded by Vaults built with the retired Liana provider
/// token service. Kept so those settings files still load and their keys keep
/// their labels; nothing creates new ones.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Provider {
    pub uuid: String,
    pub name: String,
}

/// A provider (Safety Net / Cosigner) key from the retired Liana provider
/// token service. Kept for existing Vaults only; see [`Provider`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct ProviderKey {
    pub uuid: String,
    pub token: String,
    pub provider: Provider,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeySetting {
    pub name: String,
    pub master_fingerprint: Fingerprint,
    pub provider_key: Option<ProviderKey>,
    /// Whether this key is a Border Wallet key (derived transiently from grid pattern).
    #[serde(default)]
    pub is_border_wallet: bool,
    /// For a Border Wallet key, where its Entropy Grid seed came from.
    ///
    /// Deliberately tri-state. `None` means *unrecorded*, which is every key
    /// enrolled before this field existed — not [`GridSeedSource::Independent`].
    /// Collapsing the two would stop the signing flow offering the phrase for
    /// Vaults that were built from the Cube's own seed, which is the whole
    /// reason the provenance is tracked. Absent from the file when `None`, so a
    /// Cube that has never enrolled a Border Wallet key gains no noise.
    ///
    /// Meaningless unless [`Self::is_border_wallet`]; no other key kind sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid_seed_source: Option<GridSeedSource>,
    /// The user's own statement that this signer produces replay-protected
    /// (unified-sighash) signatures on Bitcoin Blake2b. Only meaningful for a
    /// device signer — hardware firmware cannot report it, so it is the
    /// user's mark (`ReplayProtection::UserMarked`); hot keys and Border
    /// Wallet keys are capable by construction and Keychain keys are legacy
    /// until Lane B3, whatever this says. Absent from the file when unset, so
    /// a Cube that never marked a signer gains no noise. Read by
    /// [`crate::app::state::vault::signers::ReplayCapabilities`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_protected: Option<bool>,
    /// The Connect id of the COINCUBE Keychain key this is, when it is one.
    ///
    /// Recorded when the Vault is built, so the Pair panel can offer only the
    /// keys a Keychain phone can actually hold. `None` is only conclusive
    /// when [`WalletSettings::keychain_keys_recorded`] is set; otherwise the
    /// key's provenance is unrecorded. Absent from the file when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_key_id: Option<u64>,
}

/// Where a Border Wallet key's Entropy Grid seed came from.
///
/// Provenance, never secret material — the twelve words themselves are never
/// persisted. This exists so the signing flow can tell "I can re-derive those
/// words" from "I cannot", instead of guessing from whichever hot signer the
/// Vault happens to have loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GridSeedSource {
    /// BIP-85 from the signer with this **BIP-32 master fingerprint**.
    ///
    /// The fingerprint is the point. The installer derives the grid seed from
    /// the Cube master in every mode. Historical installers also used unrelated
    /// generated seeds. Neither is guaranteed to be the hot signer a later
    /// `Wallet` load turns up.
    /// `Wallet::signer` is "some descriptor key whose seed is on this machine",
    /// which can be a *different* seed entirely (a key reused from an earlier
    /// Vault) or absent while the right seed sits in the session (a master seed
    /// that was never added to the descriptor). Naming the seed makes the
    /// signing flow exact instead of hopeful.
    MasterDerived { fingerprint: Fingerprint },
    /// Random, or typed/edited by hand. Nothing on this machine re-derives it;
    /// the user holds the only copy.
    Independent,
}

impl KeySetting {
    pub fn to_backup(&self) -> Key {
        if let Some(provider_key) = &self.provider_key {
            if let Ok(metadata) = serde_json::to_value(provider_key) {
                return Key {
                    key: self.master_fingerprint,
                    alias: Some(self.name.clone()),
                    role: None,
                    key_type: Some(KeyType::ThirdParty),
                    proprietary: metadata,
                };
            }
        }
        let proprietary = if self.is_border_wallet {
            // `grid_seed_source` is omitted when unrecorded, so a restore reads
            // back `None` rather than inventing `Independent`.
            let mut metadata = serde_json::json!({ "is_border_wallet": true });
            if let Some(src) = self
                .grid_seed_source
                .and_then(|src| serde_json::to_value(src).ok())
            {
                metadata["grid_seed_source"] = src;
            }
            metadata
        } else {
            serde_json::Value::Null
        };
        Key {
            key: self.master_fingerprint,
            alias: Some(self.name.clone()),
            role: None,
            key_type: None,
            proprietary,
        }
    }

    pub fn from_backup(
        name: String,
        fg: Fingerprint,
        _role: Option<KeyRole>,
        key_type: Option<KeyType>,
        metadata: serde_json::Value,
    ) -> Option<Self> {
        if let Some(KeyType::ThirdParty) = key_type {
            let provider_key = serde_json::from_value(metadata).ok();
            Some(Self {
                name,
                master_fingerprint: fg,
                provider_key,
                is_border_wallet: false,
                grid_seed_source: None,
                replay_protected: None,
                keychain_key_id: None,
            })
        } else {
            let is_border_wallet = metadata
                .get("is_border_wallet")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let grid_seed_source = metadata
                .get("grid_seed_source")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            Some(Self {
                name,
                master_fingerprint: fg,
                provider_key: None,
                is_border_wallet,
                grid_seed_source,
                // A user's replay mark is a statement about a device on this
                // machine, not part of the Vault's backup; a restore starts
                // unmarked.
                replay_protected: None,
                // Not part of the backup; the restored Vault is unrecorded.
                keychain_key_id: None,
            })
        }
    }

    pub fn to_form(&self) -> form::Value<String> {
        form::Value {
            value: self.name.clone(),
            warning: None,
            valid: true,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn has_name(&self) -> bool {
        !self.name.is_empty()
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SettingsError {
    NotFound,
    ReadingFile(String),
    DeletingFile(String),
    WritingFile(String),
    Unexpected(String),
}
impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::DeletingFile(e) => write!(f, "Error while deleting file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

/// global settings.
#[allow(unstable_name_collisions)]
pub mod global {
    use super::preferences::{
        ArtworkChoices, DevicePreferences, Language, LauncherStyle, MotionPreference,
        NetworkActivitySource, PreferenceChange, ThemePreference, WorldviewMode, Zoom,
    };
    use crate::dir::CoincubeDirectory;
    use async_hwi::bitbox::{ConfigError, NoiseConfig, NoiseConfigData};
    use fs4::fs_std::FileExt;
    use serde::{Deserialize, Deserializer, Serialize};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};

    pub const DEFAULT_FILE_NAME: &str = "global_settings.json";

    #[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
    pub struct WindowConfig {
        pub width: f32,
        pub height: f32,
    }

    /// Subscription tier for the user's Connect account.
    ///
    /// Determines how many Cubes can be created per network.
    #[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum AccountTier {
        /// No Connect account or free tier — 2 Cubes per network.
        #[default]
        Free,
        /// Pro Connect account — 4 Cubes per network (with Lightning address & avatar).
        Pro,
        /// Estate Connect account — 7 Cubes per network (with Lightning address & avatar).
        #[serde(alias = "legacy")]
        Estate,
    }

    impl AccountTier {
        /// Maximum number of Cubes allowed per network for this tier.
        pub fn cube_limit(self) -> usize {
            match self {
                Self::Free => 2,
                Self::Pro => 4,
                Self::Estate => 7,
            }
        }

        pub fn display_name(self) -> &'static str {
            match self {
                Self::Free => "Free",
                Self::Pro => "Pro",
                Self::Estate => "Estate",
            }
        }
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct GlobalSettings {
        pub bitbox: Option<BitboxSettings>,
        pub window_config: Option<WindowConfig>,
        #[serde(default)]
        pub developer_mode: bool,
        /// Installation-wide opt-in; the Connect feature flag must also be on.
        #[serde(default)]
        pub bitcoin_blake2b_beta: bool,
        #[serde(default)]
        pub account_tier: AccountTier,
        #[serde(default)]
        pub theme_mode: coincube_ui::theme::palette::ThemeMode,
        /// `None` identifies legacy JSON. Fresh settings use `Some(System)`;
        /// explicit legacy dark/light choices migrate on the next write.
        #[serde(default, deserialize_with = "deserialize_present_theme_preference")]
        pub theme_preference: Option<ThemePreference>,
        #[serde(default)]
        pub worldview: WorldviewMode,
        #[serde(default)]
        pub language: Language,
        #[serde(default)]
        pub artwork: ArtworkChoices,
        #[serde(default)]
        pub launcher_style: LauncherStyle,
        /// Not, by itself, authority to show amounts: see
        /// `balance_privacy_initialized` and
        /// [`GlobalSettings::load_or_initialize_preferences`].
        #[serde(default)]
        pub hide_balances: bool,
        /// Durable provenance for `hide_balances`. Missing or false means the
        /// installation's privacy choice is unresolved, even when an unrelated
        /// write already serialized `hide_balances: false`. Only an explicit
        /// Hide balances choice or privacy initialization sets it; generic
        /// writers preserve it and cannot set it. Builds that predate it drop it
        /// when they rewrite the file, which returns the choice to unresolved.
        #[serde(default)]
        balance_privacy_initialized: bool,
        #[serde(default)]
        pub motion_preference: MotionPreference,
        #[serde(default)]
        pub zoom: Zoom,
        #[serde(default = "default_true")]
        pub show_direction_badges: bool,
        /// Local installation privacy preference; never part of a Cube/account record.
        #[serde(default = "default_true")]
        pub recipient_identity_checks: bool,
        /// Hardware-advisory badges the user has collapsed, as
        /// `"<fingerprint>:<advisory id>"` (see
        /// [`crate::hw_advisory::dismissals`]). Dismissal only collapses the
        /// detail panel — the badge itself stays on the device row.
        #[serde(default)]
        pub dismissed_hw_advisories: Vec<String>,
        /// Advisory ids whose one-time, app-wide incident notice has been
        /// shown and acknowledged on this install.
        #[serde(default)]
        pub seen_advisory_notices: Vec<String>,
        /// The Launcher's network activity behind the Cubes; on unless turned off.
        #[serde(default = "default_true")]
        pub network_activity: bool,
        #[serde(default)]
        pub network_activity_source: NetworkActivitySource,
    }

    fn default_true() -> bool {
        true
    }

    fn deserialize_present_theme_preference<'de, D>(
        deserializer: D,
    ) -> Result<Option<ThemePreference>, D::Error>
    where
        D: Deserializer<'de>,
    {
        ThemePreference::deserialize(deserializer).map(Some)
    }

    impl Default for GlobalSettings {
        fn default() -> Self {
            Self {
                bitbox: None,
                window_config: None,
                developer_mode: false,
                bitcoin_blake2b_beta: false,
                account_tier: AccountTier::default(),
                theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
                theme_preference: Some(ThemePreference::System),
                worldview: WorldviewMode::default(),
                language: Language::default(),
                artwork: ArtworkChoices::default(),
                launcher_style: LauncherStyle::default(),
                hide_balances: false,
                // A missing file can coexist with legacy Cubes: never resolved by default.
                balance_privacy_initialized: false,
                motion_preference: MotionPreference::default(),
                zoom: Zoom::default(),
                // Preserve the pre-existing Rust Default for this unrelated field.
                show_direction_badges: false,
                recipient_identity_checks: true,
                dismissed_hw_advisories: Vec::new(),
                seen_advisory_notices: Vec::new(),
                network_activity: true,
                network_activity_source: NetworkActivitySource::default(),
            }
        }
    }

    impl GlobalSettings {
        fn preferences(&self) -> DevicePreferences {
            DevicePreferences {
                theme: self.theme_preference.unwrap_or(self.theme_mode.into()),
                worldview: self.worldview,
                language: self.language,
                artwork: self.artwork,
                launcher_style: self.launcher_style,
                hide_balances: self.hide_balances,
                motion: self.motion_preference,
                zoom: self.zoom,
                developer_mode: self.developer_mode,
                bitcoin_blake2b_beta: self.bitcoin_blake2b_beta,
                network_activity: self.network_activity,
                network_activity_source: self.network_activity_source,
            }
        }

        fn apply_preference(&mut self, change: PreferenceChange) {
            match change {
                PreferenceChange::Theme(choice) => {
                    self.theme_preference = Some(choice);
                    // Legacy readers need a deterministic concrete value.
                    self.theme_mode = choice.resolve(None);
                }
                PreferenceChange::Worldview(mode) => self.worldview = mode,
                PreferenceChange::Language(language) => self.language = language,
                PreferenceChange::Artwork(mode, style) => self.artwork.set(mode, style),
                PreferenceChange::LauncherStyle(style) => self.launcher_style = style,
                PreferenceChange::HideBalances(hidden) => {
                    self.hide_balances = hidden;
                    // An explicit device choice is established provenance,
                    // including a false that equals the default.
                    self.balance_privacy_initialized = true;
                }
                PreferenceChange::Motion(motion) => self.motion_preference = motion,
                PreferenceChange::Zoom(zoom) => self.zoom = zoom,
                PreferenceChange::DeveloperMode(enabled) => self.developer_mode = enabled,
                PreferenceChange::BitcoinBlake2bBeta(enabled) => {
                    self.bitcoin_blake2b_beta = enabled;
                }
                PreferenceChange::NetworkActivity(shown) => self.network_activity = shown,
                PreferenceChange::NetworkActivitySource(source) => {
                    self.network_activity_source = source;
                }
            }
        }

        /// Load the device choices without writing migration data or creating
        /// any file. An absent file yields the defaults; absence proves neither
        /// a new installation nor permission to show amounts.
        ///
        /// Read-only: its `hide_balances` is **not** permission to show
        /// amounts. That takes a `PreferencesReceipt` whose `balances_hidden()`
        /// is `Some(false)`, from [`Self::load_or_initialize_preferences`] or
        /// from an explicit Show through [`Self::save_preference`].
        pub fn load_preferences(path: &Path) -> Result<DevicePreferences, super::SettingsError> {
            Self::read_settings(path).map(|settings| settings.preferences())
        }

        /// Read the current privacy choice under the global settings lock.
        /// A cached Show receipt is insufficient at render time: another
        /// process may have committed Hide since that receipt was issued.
        pub fn current_balances_hidden(path: &Path) -> Result<Option<bool>, super::SettingsError> {
            let settings = Self::read_settings(path)?;
            Ok(settings
                .balance_privacy_initialized
                .then_some(settings.hide_balances))
        }

        /// Render-time privacy check. If a writer holds the settings lock,
        /// return an unresolved choice immediately so the GUI covers amounts.
        /// Other callers can still use the strict, blocking read above.
        pub fn try_current_balances_hidden(
            path: &Path,
        ) -> Result<Option<bool>, super::SettingsError> {
            let lock = super::open_existing_lock(&Self::lock_path(path)?).map_err(|e| {
                super::SettingsError::ReadingFile(format!("Opening global settings lock: {e}"))
            })?;
            let held = match lock {
                Some(lock) => {
                    if !FileExt::try_lock_shared(&lock).map_err(|e| {
                        super::SettingsError::ReadingFile(format!("Locking global settings: {e}"))
                    })? {
                        return Ok(None);
                    }
                    Some(HeldLock(Some(lock)))
                }
                None => None,
            };
            let document = Self::read_document(path);
            drop(held);
            document.map(|document| {
                document
                    .settings
                    .balance_privacy_initialized
                    .then_some(document.settings.hide_balances)
            })
        }

        /// Edit one choice in the shared transaction and return the new
        /// snapshot once the replacement is visible. A committed save whose
        /// post-rename steps could not be confirmed is logged and still
        /// returned: the previous bytes are gone. See [`Self::save_preference`].
        pub fn update_preference(
            path: &Path,
            change: PreferenceChange,
        ) -> Result<DevicePreferences, super::SettingsError> {
            let receipt = Self::save_preference(path, change)?;
            receipt.log_unconfirmed();
            Ok(receipt.preferences())
        }

        /// Strict save boundary for the Preferences owner. `Err` always means
        /// no replacement became visible and the previous file (or its absence)
        /// remains. `Ok` carries the committed snapshot and whether every
        /// post-rename step completed. An explicit Hide balances choice also
        /// establishes the installation's privacy choice in the same write.
        pub fn save_preference(
            path: &Path,
            change: PreferenceChange,
        ) -> Result<PreferencesReceipt, super::SettingsError> {
            let force_privacy = matches!(change, PreferenceChange::HideBalances(_));
            Self::transact(path, |settings| {
                settings.apply_preference(change);
                Ok(Plan::Write {
                    value: settings.snapshot(),
                    force_privacy,
                })
            })
            .map(receipt)
        }

        /// The authoritative read of the privacy choice: returns the persisted
        /// choice once established, or establishes it exactly once. The other
        /// authority is the receipt of an explicit Show or Hide through
        /// [`Self::save_preference`], which establishes the choice itself.
        ///
        /// `root` is treated as an existing installation. While its global
        /// settings hold no established choice, this commits
        /// `hide_balances = true` together with the provenance marker (hide
        /// once on upgrade, A4), whatever the stored value says. Nothing here
        /// proves a new installation, and an empty root (for example after a
        /// wipe) is never taken as one. Showing amounts afterwards takes an
        /// explicit choice through [`Self::save_preference`]. A choice
        /// established meanwhile wins at the recheck under the global lock.
        ///
        /// Only the global settings file is read or written: Cube metadata,
        /// wallets and seeds play no part, and `root` is never created.
        ///
        /// Any `Err` means nothing was initialized: keep amounts masked and
        /// offer a retry. Concurrency covers cooperating builds only; an older
        /// build running at the same time ignores the lock and may erase the
        /// marker, which returns the installation to unresolved and so to
        /// hidden.
        pub fn load_or_initialize_preferences(
            root: &CoincubeDirectory,
        ) -> Result<PreferencesReceipt, PrivacyInitError> {
            let path = Self::path(root);
            let current = Self::read_settings(&path).map_err(PrivacyInitError::Settings)?;
            if current.balance_privacy_initialized {
                return Ok(receipt(Outcome::Kept(current.snapshot())));
            }
            Self::transact(&path, |settings| {
                if settings.balance_privacy_initialized {
                    return Ok(Plan::Keep(settings.snapshot()));
                }
                // Hide once on upgrade (A4). Only a proved new installation
                // could start shown, and no such proof exists here.
                settings.hide_balances = true;
                settings.balance_privacy_initialized = true;
                Ok(Plan::Write {
                    value: settings.snapshot(),
                    force_privacy: true,
                })
            })
            .map(receipt)
            .map_err(PrivacyInitError::Settings)
        }

        /// Startup's one-use creator proof is the only path that can initialize
        /// a missing privacy choice to shown. The caller still holds the stable
        /// parent bootstrap lock while this transaction takes the global lock.
        /// A concurrent explicit choice wins the locked recheck.
        pub(crate) fn initialize_created_preferences(
            creator: crate::dir::CreatedRoot,
        ) -> Result<PreferencesReceipt, PrivacyInitError> {
            let path = Self::path(creator.root());
            Self::transact(&path, |settings| {
                if settings.balance_privacy_initialized {
                    return Ok(Plan::Keep(settings.snapshot()));
                }
                settings.hide_balances = false;
                settings.balance_privacy_initialized = true;
                Ok(Plan::Write {
                    value: settings.snapshot(),
                    force_privacy: true,
                })
            })
            .map(receipt)
            .map_err(PrivacyInitError::Settings)
        }

        pub fn path(global_datadir: &CoincubeDirectory) -> PathBuf {
            global_datadir.path().join(DEFAULT_FILE_NAME)
        }

        pub fn load_window_config(path: &Path) -> Option<WindowConfig> {
            let mut ret = None;
            if let Err(e) = Self::update(path, |s| ret = s.window_config.clone(), false) {
                tracing::error!("Failed to load window config: {e}");
            }
            ret
        }

        pub fn update_window_config(
            path: &Path,
            window_config: &WindowConfig,
        ) -> Result<(), super::SettingsError> {
            Self::update(
                path,
                |s| s.window_config = Some(window_config.clone()),
                true,
            )
        }

        pub fn load_bitbox_settings(
            path: &Path,
        ) -> Result<Option<BitboxSettings>, super::SettingsError> {
            let mut ret = None;
            Self::update(path, |s| ret = s.bitbox.clone(), false)?;
            Ok(ret)
        }

        pub fn load_developer_mode(path: &Path) -> bool {
            let mut ret = false;
            if let Err(e) = Self::update(path, |s| ret = s.developer_mode, false) {
                tracing::error!("Failed to load developer mode setting: {e}");
            }
            ret
        }

        pub fn load_bitcoin_blake2b_beta(path: &Path) -> bool {
            let mut enabled = false;
            if let Err(e) = Self::update(path, |s| enabled = s.bitcoin_blake2b_beta, false) {
                tracing::error!("Failed to load Bitcoin Blake2b beta setting: {e}");
            }
            enabled
        }

        pub fn update_bitcoin_blake2b_beta(
            path: &Path,
            enabled: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update_preference(path, PreferenceChange::BitcoinBlake2bBeta(enabled)).map(|_| ())
        }

        pub fn load_account_tier(path: &Path) -> AccountTier {
            let mut ret = AccountTier::default();
            if let Err(e) = Self::update(path, |s| ret = s.account_tier, false) {
                tracing::error!("Failed to load account tier setting: {e}");
            }
            ret
        }

        pub fn update_account_tier(
            path: &Path,
            tier: AccountTier,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.account_tier = tier, true)
        }

        pub fn update_developer_mode(
            path: &Path,
            developer_mode: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update_preference(path, PreferenceChange::DeveloperMode(developer_mode))
                .map(|_| ())
        }

        pub fn load_show_direction_badges(path: &Path) -> bool {
            let mut ret = true;
            if let Err(e) = Self::update(path, |s| ret = s.show_direction_badges, false) {
                tracing::error!("Failed to load show_direction_badges setting: {e}");
            }
            ret
        }

        pub fn update_show_direction_badges(
            path: &Path,
            show: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.show_direction_badges = show, true)
        }

        /// Missing files and older settings default to enabled. Reading does not
        /// initialise Branta or emit any preference/check telemetry.
        pub fn load_recipient_identity_checks(path: &Path) -> bool {
            let mut enabled = true;
            let _ = Self::update(path, |s| enabled = s.recipient_identity_checks, false);
            enabled
        }

        pub fn update_recipient_identity_checks(
            path: &Path,
            enabled: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.recipient_identity_checks = enabled, true)
        }

        pub fn load_theme_mode(path: &Path) -> coincube_ui::theme::palette::ThemeMode {
            let mut ret = coincube_ui::theme::palette::ThemeMode::default();
            if let Err(e) = Self::load_preferences(path).map(|s| ret = s.theme.resolve(None)) {
                tracing::error!("Failed to load theme mode setting: {e}");
            }
            ret
        }

        pub fn update_theme_mode(
            path: &Path,
            mode: coincube_ui::theme::palette::ThemeMode,
        ) -> Result<(), super::SettingsError> {
            Self::update_preference(path, PreferenceChange::Theme(mode.into())).map(|_| ())
        }

        /// Advisory-badge dismissals, as `"<fingerprint>:<advisory id>"`.
        /// Read once at startup into the in-process mirror — see
        /// [`crate::hw_advisory::dismissals`].
        pub fn load_dismissed_hw_advisories(path: &Path) -> Vec<String> {
            let mut ret = Vec::new();
            if let Err(e) =
                Self::update(path, |s| ret.clone_from(&s.dismissed_hw_advisories), false)
            {
                tracing::error!("Failed to load dismissed hardware advisories: {e}");
            }
            ret
        }

        /// Record one dismissal. Idempotent, so a repeated dismiss of the same
        /// device/advisory pair doesn't grow the file.
        pub fn dismiss_hw_advisory(path: &Path, key: String) -> Result<(), super::SettingsError> {
            Self::update(
                path,
                |s| {
                    if !s.dismissed_hw_advisories.contains(&key) {
                        s.dismissed_hw_advisories.push(key.clone());
                    }
                },
                true,
            )
        }

        /// Whether the one-time incident notice for `advisory_id` has already
        /// been acknowledged on this install.
        pub fn advisory_notice_seen(path: &Path, advisory_id: &str) -> bool {
            let mut ret = false;
            if let Err(e) = Self::update(
                path,
                |s| ret = s.seen_advisory_notices.iter().any(|id| id == advisory_id),
                false,
            ) {
                tracing::error!("Failed to load seen advisory notices: {e}");
            }
            ret
        }

        /// Mark the one-time incident notice for `advisory_id` acknowledged.
        pub fn mark_advisory_notice_seen(
            path: &Path,
            advisory_id: &str,
        ) -> Result<(), super::SettingsError> {
            let advisory_id = advisory_id.to_string();
            Self::update(
                path,
                |s| {
                    if !s.seen_advisory_notices.contains(&advisory_id) {
                        s.seen_advisory_notices.push(advisory_id.clone());
                    }
                },
                true,
            )
        }

        pub fn update_bitbox_settings(
            path: &Path,
            bitbox: &BitboxSettings,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.bitbox = Some(bitbox.clone()), true)
        }

        pub fn update<F>(
            path: &Path,
            mut update: F,
            write: bool,
        ) -> Result<(), super::SettingsError>
        where
            F: FnMut(&mut GlobalSettings),
        {
            if !write {
                // The callback sees a private copy; nothing is written or created.
                update(&mut Self::read_settings(path)?);
                return Ok(());
            }
            let outcome = Self::transact(path, |settings| {
                update(settings);
                Ok(Plan::Write {
                    value: (),
                    force_privacy: false,
                })
            })?;
            if let Outcome::Committed {
                durability: Durability::Unconfirmed(warning),
                ..
            } = outcome
            {
                tracing::warn!("Global settings were saved, but {warning}");
            }
            Ok(())
        }

        /// The sibling lock shared by every global-settings reader and writer
        /// in this build (`global_settings.lock` for the default file).
        fn lock_path(path: &Path) -> Result<PathBuf, super::SettingsError> {
            let lock = path.with_extension("lock");
            if path.file_name().is_none() || lock == path {
                return Err(super::SettingsError::Unexpected(
                    "Global settings path has no distinct lock file".to_string(),
                ));
            }
            Ok(lock)
        }

        fn directory(path: &Path) -> &Path {
            match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent,
                _ => Path::new("."),
            }
        }

        /// Read the current settings without creating anything. When the
        /// stable lock already exists it is held shared, so a cooperating
        /// writer never replaces the file in the middle of this read. A
        /// replacement is atomic, so a reader needs no lock for consistency
        /// when no cooperating writer has ever run here.
        fn read_settings(path: &Path) -> Result<GlobalSettings, super::SettingsError> {
            let lock = super::open_existing_lock(&Self::lock_path(path)?).map_err(|e| {
                super::SettingsError::ReadingFile(format!("Opening global settings lock: {e}"))
            })?;
            let held = match lock {
                Some(lock) => {
                    FileExt::lock_shared(&lock).map_err(|e| {
                        super::SettingsError::ReadingFile(format!("Locking global settings: {e}"))
                    })?;
                    Some(HeldLock(Some(lock)))
                }
                None => None,
            };
            let document = Self::read_document(path);
            drop(held);
            document.map(|document| document.settings)
        }

        fn read_document(path: &Path) -> Result<Document, super::SettingsError> {
            use super::SettingsError::ReadingFile;
            let metadata = super::inspect_regular_file(path)
                .map_err(|e| ReadingFile(format!("Inspecting global settings file: {e}")))?;
            let Some(metadata) = metadata else {
                return Ok(Document {
                    raw: None,
                    source: Vec::new(),
                    settings: GlobalSettings::default(),
                    read_only: false,
                });
            };
            let mut file =
                File::open(path).map_err(|e| ReadingFile(format!("Opening file: {e}")))?;
            super::verify_opened_file(&file, path)
                .map_err(|e| ReadingFile(format!("Opening global settings file: {e}")))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|e| ReadingFile(format!("Reading file: {e}")))?;
            let (raw, settings) = parse_document(&bytes).map_err(ReadingFile)?;
            Ok(Document {
                raw: Some(raw),
                source: bytes,
                settings,
                read_only: metadata.permissions().readonly(),
            })
        }

        /// One read-modify-write under the stable sibling lock. The lock is
        /// taken before deciding whether the file exists, so concurrent first
        /// writers serialize and the later one rereads the earlier result. The
        /// callback runs exactly once, on the latest committed settings, and
        /// must not call back into these APIs (the lock is not reentrant).
        fn transact<T>(
            path: &Path,
            decide: impl FnOnce(&mut GlobalSettings) -> Result<Plan<T>, super::SettingsError>,
        ) -> Result<Outcome<T>, super::SettingsError> {
            use super::SettingsError::WritingFile;
            match std::fs::metadata(Self::directory(path)) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(WritingFile(
                        "Global settings directory is not a directory".to_string(),
                    ))
                }
                Err(e) => {
                    return Err(WritingFile(format!(
                        "Inspecting global settings directory: {e}"
                    )))
                }
            }
            let lock = super::open_or_create_lock(&Self::lock_path(path)?)
                .map_err(|e| WritingFile(format!("Opening global settings lock: {e}")))?;
            stage(path, Stage::BeforeLock)?;
            FileExt::lock_exclusive(&lock)
                .map_err(|e| WritingFile(format!("Locking global settings: {e}")))?;
            // Every early return from here releases the lock through `held`.
            let held = HeldLock(Some(lock));
            stage(path, Stage::Locked)?;

            let document = Self::read_document(path)?;
            let mut settings = document.settings;
            let before = to_object(&settings)?;
            // Preserve the previous explicit/missing-theme behavior in legacy
            // files, but never write migration data during a read.
            if settings.theme_preference.is_none() {
                settings.theme_preference = Some(settings.theme_mode.into());
            }
            let (value, force_privacy) = match decide(&mut settings)? {
                Plan::Keep(value) => return Ok(Outcome::Kept(value)),
                Plan::Write {
                    value,
                    force_privacy,
                } => (value, force_privacy),
            };
            if document.read_only {
                return Err(WritingFile("Global settings file is read-only".to_string()));
            }
            if document.raw.is_some() {
                // Refused before any replacement exists: the bytes stay as they are.
                require_exact_unknown_numbers(&document.source, &before).map_err(|e| {
                    WritingFile(format!("Refusing to rewrite global settings: {e}"))
                })?;
            }
            let after = to_object(&settings)?;
            let mut output = match document.raw {
                Some(mut raw) => {
                    merge_changed(&mut raw, &before, &after);
                    raw
                }
                // A first file keeps the historical full serialization, including
                // Rust Default values that differ from serde's missing-field ones.
                None => after.clone(),
            };
            if force_privacy {
                // A changed-value diff would miss an explicit false that equals
                // the stored value; the choice and its provenance travel together.
                output.insert(
                    "hide_balances".to_string(),
                    serde_json::Value::Bool(settings.hide_balances),
                );
                output.insert(
                    "balance_privacy_initialized".to_string(),
                    serde_json::Value::Bool(settings.balance_privacy_initialized),
                );
            }
            let bytes = serde_json::to_vec_pretty(&output)
                .map_err(|e| WritingFile(format!("Failed to serialize GlobalSettings: {e}")))?;
            let (_, check) = parse_document(&bytes)
                .map_err(|e| WritingFile(format!("Validating merged global settings: {e}")))?;
            if to_object(&check)? != after {
                return Err(WritingFile(
                    "Merged global settings do not decode to the intended update".to_string(),
                ));
            }

            let mut warnings = Self::replace(path, &bytes)?;
            if let Err(e) = hook(path, Stage::Unlock).and_then(|()| held.release()) {
                warnings.push(format!("releasing the settings lock failed: {e}"));
            }
            Ok(Outcome::Committed {
                value,
                durability: if warnings.is_empty() {
                    Durability::StepsCompleted
                } else {
                    Durability::Unconfirmed(warnings.join("; "))
                },
            })
        }

        /// Stage a complete replacement in the target's directory and rename it
        /// over the target. Every error is returned before the rename and leaves
        /// the previous file (or its absence) in place. After the rename the new
        /// bytes are visible; the returned warnings describe post-rename steps
        /// that could not be confirmed, never a reverted save.
        fn replace(path: &Path, bytes: &[u8]) -> Result<Vec<String>, super::SettingsError> {
            use super::SettingsError::WritingFile;
            /// Removes only the temporary file this transaction created.
            struct Staged(Option<PathBuf>);
            impl Drop for Staged {
                fn drop(&mut self) {
                    if let Some(path) = self.0.take() {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }

            let directory = Self::directory(path);
            let name = path
                .file_name()
                .ok_or_else(|| WritingFile("Global settings path has no file name".to_string()))?;
            let temporary = directory.join(format!(
                ".{}.{}.tmp",
                name.to_string_lossy(),
                uuid::Uuid::new_v4()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            // The file holds the BitBox pairing key: owner-only like Cube
            // settings, deliberately not inheriting a broader existing mode.
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            stage(path, Stage::Create)?;
            let mut file = options
                .open(&temporary)
                .map_err(|e| WritingFile(format!("Creating replacement: {e}")))?;
            let mut staged = Staged(Some(temporary.clone()));
            stage(path, Stage::Write)?;
            file.write_all(bytes)
                .map_err(|e| WritingFile(format!("Failed to write file: {e}")))?;
            stage(path, Stage::Flush)?;
            file.flush()
                .map_err(|e| WritingFile(format!("Failed to flush file: {e}")))?;
            stage(path, Stage::FileSync)?;
            file.sync_all()
                .map_err(|e| WritingFile(format!("Failed to sync file: {e}")))?;
            drop(file);
            stage(path, Stage::Rename)?;
            std::fs::rename(&temporary, path)
                .map_err(|e| WritingFile(format!("Replacing global settings: {e}")))?;
            // Commit point: the replacement is visible and no longer ours to remove.
            staged.0 = None;

            #[cfg(unix)]
            let warnings = match hook(path, Stage::DirectorySync)
                .and_then(|()| File::open(directory).and_then(|d| d.sync_all()))
            {
                Ok(()) => Vec::new(),
                Err(e) => vec![format!("syncing the settings directory failed: {e}")],
            };
            #[cfg(not(unix))]
            let warnings = Vec::new();
            Ok(warnings)
        }
    }

    /// Receipt for the strict preference boundary, kept apart from
    /// [`DevicePreferences`] so that snapshot type stays unchanged. Only this
    /// module constructs one, from a single read or locked transaction: the
    /// snapshot and its privacy provenance always describe the same bytes.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PreferencesReceipt {
        preferences: DevicePreferences,
        privacy_established: bool,
        persistence: Persistence,
    }

    /// Whether the call that produced a receipt wrote anything.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Persistence {
        /// Nothing was written; the snapshot is what the file already held.
        Unchanged,
        /// This call's replacement is visible.
        Committed(Durability),
    }

    impl PreferencesReceipt {
        /// The persisted device choices. `hide_balances` here is authority to
        /// show amounts only when [`Self::privacy_established`] is true; prefer
        /// [`Self::balances_hidden`].
        pub fn preferences(&self) -> DevicePreferences {
            self.preferences
        }

        /// Whether the installation's privacy choice was established in the
        /// same read or transaction that produced this snapshot.
        pub fn privacy_established(&self) -> bool {
            self.privacy_established
        }

        /// The authoritative Hide balances choice, or `None` while unresolved
        /// (keep amounts masked and initialize). Never an apparent default.
        pub fn balances_hidden(&self) -> Option<bool> {
            self.privacy_established
                .then_some(self.preferences.hide_balances)
        }

        pub fn persistence(&self) -> &Persistence {
            &self.persistence
        }

        fn log_unconfirmed(&self) {
            if let Persistence::Committed(Durability::Unconfirmed(warning)) = &self.persistence {
                tracing::warn!("Preferences were saved, but {warning}");
            }
        }
    }

    /// Captured with the snapshot inside the transaction that produced it.
    struct Snapshot {
        preferences: DevicePreferences,
        privacy_established: bool,
    }

    impl GlobalSettings {
        fn snapshot(&self) -> Snapshot {
            Snapshot {
                preferences: self.preferences(),
                privacy_established: self.balance_privacy_initialized,
            }
        }
    }

    fn receipt(outcome: Outcome<Snapshot>) -> PreferencesReceipt {
        let (snapshot, persistence) = match outcome {
            Outcome::Kept(snapshot) => (snapshot, Persistence::Unchanged),
            Outcome::Committed { value, durability } => (value, Persistence::Committed(durability)),
        };
        PreferencesReceipt {
            preferences: snapshot.preferences,
            privacy_established: snapshot.privacy_established,
            persistence,
        }
    }

    /// What is known after a committed replacement. Neither variant is a
    /// power-loss guarantee: no directory sync is attempted off Unix, and an
    /// injected fault or a successful rename proves nothing about the disk.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Durability {
        /// Every post-rename step this platform performs succeeded: the
        /// directory sync on Unix and the lock release.
        StepsCompleted,
        /// The new bytes are visible but a post-rename step failed. The
        /// previous bytes are gone; the save was not undone.
        Unconfirmed(String),
    }

    /// Privacy initialization did not establish a choice. Nothing was
    /// initialized; keep amounts masked and offer a retry.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PrivacyInitError {
        /// Reading or committing the global settings failed.
        Settings(super::SettingsError),
    }

    impl std::fmt::Display for PrivacyInitError {
        // Deliberately generic: never a path or an OS error string.
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match self {
                Self::Settings(_) => write!(f, "Your privacy setting could not be loaded."),
            }
        }
    }

    struct Document {
        /// The original JSON object, so unknown fields survive a rewrite.
        raw: Option<serde_json::Map<String, serde_json::Value>>,
        /// The original bytes behind `raw`; empty when the file is absent.
        source: Vec<u8>,
        settings: GlobalSettings,
        read_only: bool,
    }

    enum Plan<T> {
        /// Return `T` without writing.
        Keep(T),
        /// Commit the edited settings and return `T`.
        Write { value: T, force_privacy: bool },
    }

    enum Outcome<T> {
        Kept(T),
        Committed { value: T, durability: Durability },
    }

    /// Releases an advisory lock explicitly rather than waiting for the OS to
    /// notice the closed handle (which Windows documents as unprompt).
    struct HeldLock(Option<File>);

    impl HeldLock {
        fn release(mut self) -> std::io::Result<()> {
            match self.0.take() {
                Some(file) => FileExt::unlock(&file),
                None => Ok(()),
            }
        }
    }

    impl Drop for HeldLock {
        fn drop(&mut self) {
            if let Some(file) = self.0.take() {
                let _ = FileExt::unlock(&file);
            }
        }
    }

    /// Decode a document strictly: one JSON object, no repeated keys at any
    /// depth, and every known field valid. Empty and truncated input fail.
    fn parse_document(
        bytes: &[u8],
    ) -> Result<(serde_json::Map<String, serde_json::Value>, GlobalSettings), String> {
        serde_json::from_slice::<super::UniqueKeys>(bytes).map_err(|e| e.to_string())?;
        let serde_json::Value::Object(raw) =
            serde_json::from_slice::<serde_json::Value>(bytes).map_err(|e| e.to_string())?
        else {
            return Err("Global settings must be a JSON object".to_string());
        };
        let settings = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        // An established marker is authority only together with the choice it
        // records; a missing value would otherwise read as the default false.
        if raw.get("balance_privacy_initialized") == Some(&serde_json::Value::Bool(true))
            && !matches!(raw.get("hide_balances"), Some(serde_json::Value::Bool(_)))
        {
            return Err(
                "Global settings mark the privacy choice established without recording it"
                    .to_string(),
            );
        }
        Ok((raw, settings))
    }

    fn to_object(
        settings: &GlobalSettings,
    ) -> Result<serde_json::Map<String, serde_json::Value>, super::SettingsError> {
        match serde_json::to_value(settings) {
            Ok(serde_json::Value::Object(map)) => Ok(map),
            Ok(_) => Err(super::SettingsError::Unexpected(
                "GlobalSettings did not serialize to an object".to_string(),
            )),
            Err(e) => Err(super::SettingsError::WritingFile(format!(
                "Failed to serialize GlobalSettings: {e}"
            ))),
        }
    }

    /// Write back only the known paths whose typed value changed. Unknown
    /// top-level and nested fields, and known fields left at their serde
    /// default by omission, keep their original representation.
    fn merge_changed(
        raw: &mut serde_json::Map<String, serde_json::Value>,
        before: &serde_json::Map<String, serde_json::Value>,
        after: &serde_json::Map<String, serde_json::Value>,
    ) {
        use serde_json::Value::Object;
        for (key, new) in after {
            let old = before.get(key);
            if old == Some(new) {
                continue;
            }
            if let (Some(Object(raw_child)), Some(Object(old)), Object(new)) =
                (raw.get_mut(key), old, new)
            {
                merge_changed(raw_child, old, new);
                continue;
            }
            raw.insert(key.clone(), new.clone());
        }
        for key in before.keys() {
            if !after.contains_key(key) {
                raw.remove(key);
            }
        }
    }

    /// Refuses a rewrite that would silently change an unknown number.
    /// Without serde_json's `arbitrary_precision`, `Value` rounds integers
    /// beyond i64/u64 and decimals beyond f64 to an f64, and the raw-preserving
    /// merge would write that rounded value back. So every number token in the
    /// original bytes outside the typed settings (`known`, as [`to_object`]
    /// serializes them) must come back from `Value` with exactly the same
    /// decimal value; only its spelling may change. Known fields are written
    /// from their typed value, as before.
    fn require_exact_unknown_numbers(
        bytes: &[u8],
        known: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        NumberScan {
            bytes,
            at: 0,
            path: Vec::new(),
            known,
        }
        .value()
    }

    /// One step from the document root.
    enum Step {
        Key(String),
        Index(usize),
    }

    /// A walk over bytes that [`parse_document`] already accepted (so nesting
    /// is within serde_json's recursion limit). It only tracks structure and
    /// refuses anything it does not expect rather than guessing.
    struct NumberScan<'a> {
        bytes: &'a [u8],
        at: usize,
        path: Vec<Step>,
        known: &'a serde_json::Map<String, serde_json::Value>,
    }

    impl NumberScan<'_> {
        fn unexpected() -> String {
            "Global settings could not be checked for exact numbers".to_string()
        }

        fn peek(&mut self) -> Option<u8> {
            while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.at += 1;
            }
            self.bytes.get(self.at).copied()
        }

        /// Consumes `close` if it ends the container, or a separator.
        fn next_item(&mut self, close: u8) -> Result<bool, String> {
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    Ok(true)
                }
                Some(byte) if byte == close => {
                    self.at += 1;
                    Ok(false)
                }
                _ => Err(Self::unexpected()),
            }
        }

        fn value(&mut self) -> Result<(), String> {
            match self.peek() {
                Some(b'{') => {
                    self.at += 1;
                    if self.peek() == Some(b'}') {
                        self.at += 1;
                        return Ok(());
                    }
                    loop {
                        let key = self.string()?;
                        if self.peek() != Some(b':') {
                            return Err(Self::unexpected());
                        }
                        self.at += 1;
                        self.path.push(Step::Key(key));
                        self.value()?;
                        self.path.pop();
                        if !self.next_item(b'}')? {
                            return Ok(());
                        }
                    }
                }
                Some(b'[') => {
                    self.at += 1;
                    if self.peek() == Some(b']') {
                        self.at += 1;
                        return Ok(());
                    }
                    let mut index = 0;
                    loop {
                        self.path.push(Step::Index(index));
                        self.value()?;
                        self.path.pop();
                        index += 1;
                        if !self.next_item(b']')? {
                            return Ok(());
                        }
                    }
                }
                Some(b'"') => self.string().map(drop),
                Some(b'-' | b'0'..=b'9') => self.number(),
                Some(b't' | b'f' | b'n') => {
                    while matches!(self.bytes.get(self.at), Some(b'a'..=b'z')) {
                        self.at += 1;
                    }
                    Ok(())
                }
                _ => Err(Self::unexpected()),
            }
        }

        fn string(&mut self) -> Result<String, String> {
            if self.peek() != Some(b'"') {
                return Err(Self::unexpected());
            }
            let start = self.at;
            self.at += 1;
            loop {
                match self.bytes.get(self.at) {
                    Some(b'"') => break,
                    Some(b'\\') => self.at += 2,
                    Some(_) => self.at += 1,
                    None => return Err(Self::unexpected()),
                }
            }
            self.at += 1;
            serde_json::from_slice(&self.bytes[start..self.at]).map_err(|e| e.to_string())
        }

        fn number(&mut self) -> Result<(), String> {
            let start = self.at;
            while matches!(
                self.bytes.get(self.at),
                Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            ) {
                self.at += 1;
            }
            let token =
                std::str::from_utf8(&self.bytes[start..self.at]).map_err(|e| e.to_string())?;
            // A field that accepts malformed input may normalize a number to
            // a nonnumeric typed value. The raw token still needs protection.
            if known_value(self.known, &self.path).is_some_and(serde_json::Value::is_number)
                || number_survives(token)
            {
                Ok(())
            } else {
                Err("Global settings hold a number this build cannot keep exactly".to_string())
            }
        }
    }

    /// The typed value at `path`, when the settings schema knows that path.
    fn known_value<'a>(
        known: &'a serde_json::Map<String, serde_json::Value>,
        path: &[Step],
    ) -> Option<&'a serde_json::Value> {
        let (Step::Key(first), rest) = path.split_first()? else {
            return None;
        };
        let mut value = known.get(first)?;
        for step in rest {
            value = match (step, value) {
                (Step::Key(key), serde_json::Value::Object(map)) => map.get(key)?,
                (Step::Index(index), serde_json::Value::Array(items)) => items.get(*index)?,
                _ => return None,
            };
        }
        Some(value)
    }

    /// Whether the number `token` reads back from `serde_json::Value` with the
    /// same exact decimal value. The comparison is on decimal digits, never on
    /// two already-rounded parses.
    fn number_survives(token: &str) -> bool {
        let Ok(serde_json::Value::Number(parsed)) =
            serde_json::from_str::<serde_json::Value>(token)
        else {
            return false;
        };
        match (exact_decimal(token), exact_decimal(&parsed.to_string())) {
            (Some(original), Some(kept)) => original == kept,
            _ => false,
        }
    }

    /// A JSON number's exact value as (negative, significant digits without
    /// leading or trailing zeros, power of ten). Zero has a single form,
    /// whatever its sign or spelling. `None` for anything else.
    fn exact_decimal(token: &str) -> Option<(bool, String, i128)> {
        let (negative, unsigned) = match token.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, token),
        };
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(at) => (&unsigned[..at], unsigned[at + 1..].parse::<i64>().ok()?),
            None => (unsigned, 0),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let digits = format!("{whole}{fraction}");
        let digits = digits.trim_start_matches('0');
        let significant = digits.trim_end_matches('0');
        if significant.is_empty() {
            return Some((false, String::new(), 0));
        }
        let trailing = (digits.len() - significant.len()) as i128;
        Some((
            negative,
            significant.to_string(),
            i128::from(exponent) - fraction.len() as i128 + trailing,
        ))
    }

    /// Points in a transaction where tests pause or inject a failure.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Stage {
        BeforeLock,
        Locked,
        Create,
        Write,
        Flush,
        FileSync,
        Rename,
        DirectorySync,
        Unlock,
    }

    fn stage(path: &Path, stage: Stage) -> Result<(), super::SettingsError> {
        hook(path, stage).map_err(|e| super::SettingsError::WritingFile(format!("{stage:?}: {e}")))
    }

    #[cfg(not(test))]
    #[inline(always)]
    fn hook(_: &Path, _: Stage) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    type Hook = std::sync::Arc<dyn Fn(Stage) -> std::io::Result<()> + Send + Sync>;

    #[cfg(test)]
    static HOOKS: std::sync::Mutex<Vec<(PathBuf, Hook)>> = std::sync::Mutex::new(Vec::new());

    /// Test hooks are keyed by settings path so parallel tests never share one.
    #[cfg(test)]
    fn hook(path: &Path, stage: Stage) -> std::io::Result<()> {
        let hook = HOOKS
            .lock()
            .unwrap()
            .iter()
            .find(|(hooked, _)| hooked == path)
            .map(|(_, hook)| hook.clone());
        match hook {
            Some(hook) => hook(stage),
            None => Ok(()),
        }
    }

    #[derive(Debug, Deserialize, Serialize, Clone)]
    pub struct BitboxSettings {
        pub noise_config: NoiseConfigData,
    }

    pub struct PersistedBitboxNoiseConfig {
        file_path: PathBuf,
    }

    impl async_hwi::bitbox::api::Threading for PersistedBitboxNoiseConfig {}

    impl PersistedBitboxNoiseConfig {
        /// Creates a new persisting noise config, which stores the pairing information in "bitbox.json"
        /// in the provided directory.
        pub fn new(global_datadir: &CoincubeDirectory) -> PersistedBitboxNoiseConfig {
            PersistedBitboxNoiseConfig {
                file_path: GlobalSettings::path(global_datadir),
            }
        }
    }

    impl NoiseConfig for PersistedBitboxNoiseConfig {
        fn read_config(&self) -> Result<NoiseConfigData, ConfigError> {
            let res = GlobalSettings::load_bitbox_settings(&self.file_path)
                .map_err(|e| ConfigError(e.to_string()))?
                .map(|s| s.noise_config)
                .unwrap_or_else(NoiseConfigData::default);
            Ok(res)
        }

        fn store_config(&self, conf: &NoiseConfigData) -> Result<(), ConfigError> {
            GlobalSettings::update(
                &self.file_path,
                |s| {
                    if let Some(bitbox) = s.bitbox.as_mut() {
                        bitbox.noise_config = conf.clone();
                    } else {
                        s.bitbox = Some(BitboxSettings {
                            noise_config: conf.clone(),
                        });
                    }
                },
                true,
            )
            .map_err(|e| ConfigError(e.to_string()))
        }
    }

    /// Storage-boundary tests through the production APIs. Ordering uses
    /// path-keyed hooks, and every wait is bounded: a paused transaction gives
    /// up once its test stops waiting, workers and child processes are
    /// collected against a deadline, and a child that outlives it is killed
    /// and reaped. A fixed sleep appears only after a contender has reported
    /// reaching its lock attempt, to detect one that should be blocked but is
    /// not, never to order the steps.
    #[cfg(test)]
    mod transaction_tests {
        use super::super::{open_or_create_lock, SettingsError, SETTINGS_FILE_NAME};
        use super::*;
        use crate::chain::ChainId;
        use serde_json::{json, Value};
        use std::ffi::OsStr;
        use std::process::{Child, Command, ExitStatus};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, Mutex};
        use std::time::{Duration, Instant};

        /// Long enough that a writer which ignored a lock would have finished.
        const BLOCKED: Duration = Duration::from_millis(400);
        /// Upper bound on every wait, turning a deadlock into a failure.
        const DEADLOCK: Duration = Duration::from_secs(20);
        /// Prefix of every harness failure, so the probes can recognize them.
        const HARNESS: &str = "R04B-HARNESS";

        struct TempRoot(PathBuf);

        impl TempRoot {
            fn new(tag: &str) -> Self {
                let path = std::env::temp_dir()
                    .join(format!("coincube-r04b-{tag}-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            fn root(&self) -> CoincubeDirectory {
                CoincubeDirectory::new(self.0.clone())
            }
            fn global(&self) -> PathBuf {
                self.0.join(DEFAULT_FILE_NAME)
            }
            fn chain(&self, chain: ChainId) -> PathBuf {
                self.0.join(chain.dir_name())
            }
        }

        impl Drop for TempRoot {
            fn drop(&mut self) {
                #[cfg(unix)]
                restore_permissions(&self.0);
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[cfg(unix)]
        fn restore_permissions(path: &Path) {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::symlink_metadata(path) {
                if metadata.file_type().is_symlink() {
                    return;
                }
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
                if metadata.is_dir() {
                    for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                        restore_permissions(&entry.path());
                    }
                }
            }
        }

        /// Whether permission bits actually restrict this process (not root).
        #[cfg(unix)]
        fn permissions_enforced(root: &Path) -> bool {
            use std::os::unix::fs::PermissionsExt;
            let probe = root.join("permission-probe");
            std::fs::write(&probe, b"x").unwrap();
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000)).unwrap();
            let enforced = std::fs::read(&probe).is_err();
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::remove_file(&probe).unwrap();
            enforced
        }

        struct HookGuard(PathBuf);

        impl Drop for HookGuard {
            fn drop(&mut self) {
                HOOKS.lock().unwrap().retain(|(path, _)| path != &self.0);
            }
        }

        fn install(
            path: &Path,
            hook: impl Fn(Stage) -> std::io::Result<()> + Send + Sync + 'static,
        ) -> HookGuard {
            HOOKS
                .lock()
                .unwrap()
                .push((path.to_path_buf(), Arc::new(hook)));
            HookGuard(path.to_path_buf())
        }

        /// The first transaction reaching a stage reports its arrival and
        /// waits for [`Pause::release`]. It gives up, failing that transaction
        /// and so releasing its locks, once `limit` passes or the `Pause` is
        /// dropped (for example by a failed assertion).
        struct Pause {
            arrived: mpsc::Receiver<()>,
            release: Option<mpsc::Sender<()>>,
            limit: Duration,
        }

        fn pause_once(at: Stage) -> (Pause, impl Fn(Stage) -> std::io::Result<()> + Send + Sync) {
            pause_within(at, DEADLOCK)
        }

        fn pause_within(
            at: Stage,
            limit: Duration,
        ) -> (Pause, impl Fn(Stage) -> std::io::Result<()> + Send + Sync) {
            let (arrive, arrived) = mpsc::channel();
            let (release, released) = mpsc::channel::<()>();
            let released = Mutex::new(released);
            let fired = AtomicBool::new(false);
            let hook = move |stage: Stage| {
                if stage != at || fired.swap(true, Ordering::SeqCst) {
                    return Ok(());
                }
                let _ = arrive.send(());
                match released.lock().unwrap().recv_timeout(limit) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(std::io::Error::other(format!(
                        "{HARNESS}: the paused transaction was not released"
                    ))),
                }
            };
            let pause = Pause {
                arrived,
                release: Some(release),
                limit,
            };
            (pause, hook)
        }

        impl Pause {
            /// Waits for the paused transaction. Fails as soon as `worker`
            /// finishes without arriving, and once the limit passes.
            fn arrived<T: std::fmt::Debug>(&self, worker: &Worker<T>) {
                let deadline = Instant::now() + self.limit;
                loop {
                    match self.arrived.recv_timeout(Duration::from_millis(10)) {
                        Ok(()) => return,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            panic!("{}: the pause hook is gone", HARNESS)
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if let Ok(result) = worker.result.try_recv() {
                        panic!(
                            "{}: worker finished before reaching the pause: {:?}",
                            HARNESS, result
                        );
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{HARNESS}: worker did not reach the pause within {:?}",
                        self.limit
                    );
                }
            }

            fn release(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
            }
        }

        /// A thread running one transaction, collected against a deadline.
        /// On timeout the thread is abandoned rather than joined: the test
        /// fails, and the thread ends with the test process.
        struct Worker<T> {
            result: mpsc::Receiver<std::thread::Result<T>>,
            limit: Duration,
        }

        fn worker<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> Worker<T> {
            worker_within(DEADLOCK, run)
        }

        fn worker_within<T: Send + 'static>(
            limit: Duration,
            run: impl FnOnce() -> T + Send + 'static,
        ) -> Worker<T> {
            let (done, result) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = done.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)));
            });
            Worker { result, limit }
        }

        impl<T> Worker<T> {
            /// The worker's value; a panic inside it is raised again here.
            fn join(self) -> T {
                match self.result.recv_timeout(self.limit) {
                    Ok(Ok(value)) => value,
                    Ok(Err(panic)) => std::panic::resume_unwind(panic),
                    Err(_) => panic!("{HARNESS}: worker did not finish within {:?}", self.limit),
                }
            }

            /// Fails the test if the worker has already finished.
            fn assert_blocked(&self, what: &str) {
                assert!(
                    self.result.try_recv().is_err(),
                    "{} finished while it should have been blocked",
                    what
                );
            }
        }

        /// Reports every transaction reaching `at`, such as its lock attempt.
        fn arrivals(
            at: Stage,
        ) -> (
            mpsc::Receiver<()>,
            impl Fn(Stage) -> std::io::Result<()> + Send + Sync,
        ) {
            let (arrive, arrived) = mpsc::channel();
            let hook = move |stage: Stage| {
                if stage == at {
                    let _ = arrive.send(());
                }
                Ok(())
            };
            (arrived, hook)
        }

        /// Waits, bounded, for the next arrival reported by [`arrivals`].
        fn next_arrival(arrived: &mpsc::Receiver<()>, what: &str) {
            assert!(
                arrived.recv_timeout(DEADLOCK).is_ok(),
                "{}: {} did not reach its lock attempt within {:?}",
                HARNESS,
                what,
                DEADLOCK
            );
        }

        /// A child process running one test of this binary, with its output
        /// in `log`. Dropping it kills and reaps the child, so a failing
        /// parent never leaves one running.
        struct ChildGuard(Option<Child>);

        impl ChildGuard {
            fn spawn(test: &str, env: &[(&str, &OsStr)], log: &Path) -> Self {
                let output = std::fs::File::create(log).unwrap();
                let errors = output.try_clone().unwrap();
                let child = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", test, "--nocapture", "--test-threads=1"])
                    .envs(env.iter().copied())
                    .stdout(output)
                    .stderr(errors)
                    .spawn()
                    .unwrap();
                Self(Some(child))
            }

            /// `Ok(status)` if the child exits within `limit`; otherwise it
            /// is killed and reaped, and the reaped status is the `Err`.
            fn wait(mut self, limit: Duration) -> Result<ExitStatus, ExitStatus> {
                let deadline = Instant::now() + limit;
                while Instant::now() < deadline {
                    let child = self.0.as_mut().expect("the child was already collected");
                    if let Some(status) = child.try_wait().unwrap() {
                        self.0 = None;
                        return Ok(status);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let mut child = self.0.take().expect("the child was already collected");
                let _ = child.kill();
                Err(child.wait().unwrap())
            }
        }

        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Some(mut child) = self.0.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }

        /// The libtest name of a test in this module.
        fn this_test(name: &str) -> String {
            format!("{}::{name}", module_path!().split_once("::").unwrap().1)
        }

        fn fail_at(at: Stage) -> impl Fn(Stage) -> std::io::Result<()> {
            move |stage| {
                if stage == at {
                    Err(std::io::Error::other("injected fault"))
                } else {
                    Ok(())
                }
            }
        }

        fn raw(path: &Path) -> Value {
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
        }

        /// Owned temporary replacements left in a directory.
        fn temporaries(directory: &Path) -> Vec<String> {
            std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".tmp"))
                .collect()
        }

        /// Writes legacy Cube metadata exactly as given and returns its bytes.
        fn legacy(root: &TempRoot, chain: ChainId, cubes: &[(&str, bool)]) -> Vec<u8> {
            let directory = root.chain(chain);
            std::fs::create_dir_all(&directory).unwrap();
            let cubes: Vec<Value> = cubes
                .iter()
                .map(|(id, masked)| {
                    json!({
                        "id": id,
                        "name": format!("Private label {id}"),
                        "network": chain.dir_name(),
                        "created_at": 1_720_000_000,
                        "balance_masked": masked,
                    })
                })
                .collect();
            let bytes =
                serde_json::to_vec_pretty(&json!({ "cubes": cubes, "wallets": [] })).unwrap();
            std::fs::write(directory.join(SETTINGS_FILE_NAME), &bytes).unwrap();
            bytes
        }

        fn initialize(root: &TempRoot) -> Result<PreferencesReceipt, PrivacyInitError> {
            GlobalSettings::load_or_initialize_preferences(&root.root())
        }

        fn hidden(root: &TempRoot) -> Option<bool> {
            initialize(root).unwrap().balances_hidden()
        }

        fn window() -> WindowConfig {
            WindowConfig {
                width: 1024.0,
                height: 768.0,
            }
        }

        // --- Global transaction -------------------------------------------

        #[test]
        fn concurrent_first_writers_serialize_and_keep_both_fields() {
            let root = TempRoot::new("first-file");
            let path = root.global();
            let (mut pause, pause_hook) = pause_once(Stage::Rename);
            let (attempts, attempt_hook) = arrivals(Stage::BeforeLock);
            let locked = Arc::new(Mutex::new(0usize));
            let counter = locked.clone();
            let _hook = install(&path, move |stage| {
                if stage == Stage::Locked {
                    *counter.lock().unwrap() += 1;
                }
                attempt_hook(stage)?;
                pause_hook(stage)
            });
            let first_path = path.clone();
            let first =
                worker(move || GlobalSettings::update_window_config(&first_path, &window()));
            next_arrival(&attempts, "the first writer");
            pause.arrived(&first); // first writer holds the lock with its replacement staged
            assert!(!path.exists(), "nothing is visible before the rename");
            let second_path = path.clone();
            let second =
                worker(move || GlobalSettings::update_account_tier(&second_path, AccountTier::Pro));
            next_arrival(&attempts, "the second writer");
            std::thread::sleep(BLOCKED);
            assert_eq!(
                *locked.lock().unwrap(),
                1,
                "the second first-writer entered the transaction while the first held the lock"
            );
            second.assert_blocked("the second first-writer");
            pause.release();
            first.join().unwrap();
            second.join().unwrap();
            let saved = raw(&path);
            assert_eq!(saved["account_tier"], "pro");
            assert_eq!(saved["window_config"]["width"], 1024.0);
        }

        #[test]
        fn concurrent_existing_file_writers_keep_nested_and_list_changes() {
            let root = TempRoot::new("existing-concurrent");
            let path = root.global();
            std::fs::write(&path, br#"{"dismissed_hw_advisories":["a:1"]}"#).unwrap();
            let (mut pause, pause_hook) = pause_once(Stage::Rename);
            let (attempts, attempt_hook) = arrivals(Stage::BeforeLock);
            let _hook = install(&path, move |stage| {
                attempt_hook(stage)?;
                pause_hook(stage)
            });
            let first_path = path.clone();
            let first =
                worker(move || GlobalSettings::dismiss_hw_advisory(&first_path, "b:2".to_string()));
            next_arrival(&attempts, "the first writer");
            pause.arrived(&first);
            let second_path = path.clone();
            let second = worker(move || {
                GlobalSettings::update(
                    &second_path,
                    |s| {
                        s.bitbox = Some(
                            serde_json::from_value(json!({
                                "noise_config": {
                                    "app_static_privkey": vec![7u8; 32],
                                    "device_static_pubkeys": []
                                }
                            }))
                            .unwrap(),
                        )
                    },
                    true,
                )
            });
            next_arrival(&attempts, "the second writer");
            std::thread::sleep(BLOCKED);
            second.assert_blocked("the second writer");
            pause.release();
            first.join().unwrap();
            second.join().unwrap();
            let saved = raw(&path);
            assert_eq!(saved["dismissed_hw_advisories"], json!(["a:1", "b:2"]));
            assert_eq!(saved["bitbox"]["noise_config"]["app_static_privkey"][0], 7);
        }

        #[test]
        fn unknown_and_nested_fields_survive_unrelated_writes() {
            let root = TempRoot::new("unknown-fields");
            let path = root.global();
            std::fs::write(
                &path,
                serde_json::to_vec(&json!({
                    "future_top_level": {"kept": [1, 2, {"deep": true}]},
                    "account_tier": "legacy",
                    "bitbox": {
                        "future_bitbox_field": "kept",
                        "noise_config": {
                            "app_static_privkey": vec![1u8; 32],
                            "device_static_pubkeys": [],
                            "future_noise_field": 9
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            GlobalSettings::update_window_config(&path, &window()).unwrap();
            let saved = raw(&path);
            assert_eq!(
                saved["future_top_level"],
                json!({"kept": [1, 2, {"deep": true}]})
            );
            // An unchanged known field keeps its original (alias) spelling.
            assert_eq!(saved["account_tier"], "legacy");
            assert_eq!(saved["bitbox"]["future_bitbox_field"], "kept");
            assert_eq!(saved["bitbox"]["noise_config"]["future_noise_field"], 9);
            assert_eq!(saved["window_config"]["height"], 768.0);
            assert!(saved.get("balance_privacy_initialized").is_none());

            // Changing the nested known object still keeps its unknown siblings.
            let changed: NoiseConfigData = serde_json::from_value(json!({
                "app_static_privkey": vec![2u8; 32],
                "device_static_pubkeys": []
            }))
            .unwrap();
            PersistedBitboxNoiseConfig::new(&root.root())
                .store_config(&changed)
                .unwrap();
            let saved = raw(&path);
            assert_eq!(saved["bitbox"]["noise_config"]["app_static_privkey"][0], 2);
            assert_eq!(saved["bitbox"]["noise_config"]["future_noise_field"], 9);
            assert_eq!(saved["bitbox"]["future_bitbox_field"], "kept");
            assert_eq!(saved["future_top_level"]["kept"][2]["deep"], true);
        }

        #[test]
        fn default_and_serde_default_distinctions_are_preserved() {
            let root = TempRoot::new("defaults");
            // A first file keeps the historical full Rust Default serialization.
            let fresh = root.global();
            GlobalSettings::update_window_config(&fresh, &window()).unwrap();
            assert_eq!(raw(&fresh)["show_direction_badges"], false);
            assert!(!GlobalSettings::load_show_direction_badges(&fresh));
            assert_eq!(raw(&fresh)["balance_privacy_initialized"], false);
            // An existing file leaves an omitted field to its serde default.
            let existing = root.0.join("existing.json");
            std::fs::write(&existing, b"{}").unwrap();
            GlobalSettings::update_window_config(&existing, &window()).unwrap();
            assert!(raw(&existing).get("show_direction_badges").is_none());
            assert!(GlobalSettings::load_show_direction_badges(&existing));
            // Legacy theme presence still migrates only on a write.
            std::fs::write(&existing, br#"{"theme_mode":"light"}"#).unwrap();
            GlobalSettings::update_account_tier(&existing, AccountTier::Pro).unwrap();
            assert_eq!(raw(&existing)["theme_preference"], "light");
        }

        #[test]
        fn malformed_or_ambiguous_documents_are_refused_and_never_rewritten() {
            let root = TempRoot::new("strict");
            let path = root.global();
            for document in [
                "",
                "   ",
                "{",
                r#"{"window_config":"#,
                "[]",
                r#""text""#,
                "{} {}",
                r#"{"hide_balances":"yes"}"#,
                r#"{"hide_balances":null}"#,
                r#"{"balance_privacy_initialized":1}"#,
                r#"{"balance_privacy_initialized":null}"#,
                r#"{"developer_mode":true,"developer_mode":false}"#,
                r#"{"future":{"a":1,"a":2}}"#,
                r#"{"seen_advisory_notices":[{"x":1,"x":1}]}"#,
                r#"{"theme_preference":"sepia"}"#,
                r#"{"account_tier":"platinum"}"#,
            ] {
                std::fs::write(&path, document).unwrap();
                assert!(
                    GlobalSettings::load_preferences(&path).is_err(),
                    "{:?}",
                    document
                );
                assert!(
                    GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(true))
                        .is_err(),
                    "{:?}",
                    document
                );
                assert!(GlobalSettings::update_window_config(&path, &window()).is_err());
                assert!(matches!(
                    initialize(&root),
                    Err(PrivacyInitError::Settings(_))
                ));
                assert_eq!(std::fs::read_to_string(&path).unwrap(), document);
                assert!(temporaries(&root.0).is_empty());
            }
        }

        #[test]
        fn an_established_marker_without_its_choice_is_refused_unchanged() {
            for document in [
                r#"{"balance_privacy_initialized":true}"#,
                r#"{"balance_privacy_initialized":true,"window_config":{"width":800.0,"height":600.0},"future":{"nested":[1,{"deep":true}]}}"#,
                // Only the top-level choice counts.
                r#"{"future":{"hide_balances":false},"balance_privacy_initialized":true}"#,
            ] {
                let root = TempRoot::new("marker-only");
                let path = root.global();
                std::fs::write(&path, document).unwrap();
                assert!(
                    GlobalSettings::load_preferences(&path).is_err(),
                    "{}",
                    document
                );
                assert!(GlobalSettings::update_window_config(&path, &window()).is_err());
                for change in [
                    PreferenceChange::HideBalances(false),
                    PreferenceChange::HideBalances(true),
                    PreferenceChange::DeveloperMode(true),
                ] {
                    // No receipt, so nothing can expose an apparent Some(false).
                    assert!(
                        GlobalSettings::save_preference(&path, change).is_err(),
                        "{} {:?}",
                        document,
                        change
                    );
                }
                assert!(matches!(
                    initialize(&root),
                    Err(PrivacyInitError::Settings(_))
                ));
                assert_eq!(std::fs::read_to_string(&path).unwrap(), document);
                assert!(temporaries(&root.0).is_empty(), "{}", document);
            }
            // Complete pairs keep their value; an unset marker still follows A4.
            for (document, expected) in [
                (
                    r#"{"balance_privacy_initialized":true,"hide_balances":false}"#,
                    false,
                ),
                (
                    r#"{"hide_balances":true,"balance_privacy_initialized":true}"#,
                    true,
                ),
                (r#"{"balance_privacy_initialized":false}"#, true),
                (r#"{"hide_balances":false}"#, true),
            ] {
                let root = TempRoot::new("marker-pairs");
                std::fs::write(root.global(), document).unwrap();
                assert_eq!(hidden(&root), Some(expected), "{document}");
                GlobalSettings::update_window_config(&root.global(), &window()).unwrap();
                assert_eq!(hidden(&root), Some(expected), "{document}");
                let saved = raw(&root.global());
                assert_eq!(saved["hide_balances"], expected, "{document}");
                assert_eq!(saved["balance_privacy_initialized"], true, "{document}");
            }
        }

        #[test]
        fn unknown_numbers_that_cannot_be_kept_exactly_refuse_every_write() {
            let lossy = [
                "18446744073709551616", // u64::MAX + 1
                "-9223372036854775809", // i64::MIN - 1
                "123456789012345678901234567890",
                "0.12345678901234567890123456789",
                // More digits than the nearest f64 (0.1) keeps.
                "-0.1000000000000000055511151231257827",
                "9007199254740993.0", // 2^53 + 1 as a float
                "1.00000000000000000001e10",
                "1e-400", // underflows to zero
                "4.9e-324",
                "-4.9e-324",
            ];
            for token in lossy {
                for (shape, document) in [
                    ("top level", format!(r#"{{"future":{token}}}"#)),
                    (
                        "nested map",
                        format!(r#"{{"future":{{"deeper":{{"value":{token}}}}}}}"#),
                    ),
                    (
                        "nested array",
                        format!(r#"{{"developer_mode":true,"future":[0,[{token}]]}}"#),
                    ),
                    (
                        "beside a changed window field",
                        format!(
                            r#"{{"window_config":{{"width":800.0,"height":600.0,"scale":{token}}}}}"#
                        ),
                    ),
                ] {
                    let root = TempRoot::new("lossy-number");
                    let path = root.global();
                    std::fs::write(&path, &document).unwrap();
                    let case = format!("{shape}: {token}");
                    // Reading loses nothing, so it is not refused.
                    assert!(GlobalSettings::load_preferences(&path).is_ok(), "{}", case);
                    assert!(
                        GlobalSettings::update_window_config(&path, &window()).is_err(),
                        "{}",
                        case
                    );
                    assert!(
                        GlobalSettings::update_account_tier(&path, AccountTier::Pro).is_err(),
                        "{}",
                        case
                    );
                    for hide in [true, false] {
                        assert!(
                            GlobalSettings::save_preference(
                                &path,
                                PreferenceChange::HideBalances(hide)
                            )
                            .is_err(),
                            "{}",
                            case
                        );
                    }
                    assert!(
                        matches!(initialize(&root), Err(PrivacyInitError::Settings(_))),
                        "{}",
                        case
                    );
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), document, "{case}");
                    assert!(temporaries(&root.0).is_empty(), "{}", case);
                }
            }

            // Values that come back exactly still save; only the spelling may change.
            for (token, written) in [
                ("0", "0"),
                ("42", "42"),
                ("-42", "-42"),
                ("18446744073709551615", "18446744073709551615"),
                ("-9223372036854775808", "-9223372036854775808"),
                ("0.5", "0.5"),
                ("-2.25", "-2.25"),
                ("123.456", "123.456"),
                ("1024.0", "1024.0"),
                ("1e3", "1000.0"),
                ("2.5E-3", "0.0025"),
            ] {
                let root = TempRoot::new("exact-number");
                let path = root.global();
                let document = format!(
                    r#"{{"future":{{"value":{token},"list":[{token}]}},"window_config":{{"width":1366.5,"height":768.25}}}}"#
                );
                std::fs::write(&path, &document).unwrap();
                GlobalSettings::update_window_config(&path, &window()).unwrap();
                assert_eq!(hidden(&root), Some(true), "{token}");
                GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false))
                    .unwrap();
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    text.contains(&format!("\"value\": {written}")),
                    "{}: {}",
                    token,
                    text
                );
                assert_eq!(raw(&path)["window_config"]["width"], 1024.0, "{token}");
            }

            // Known typed fields are written from their typed value, as before,
            // so an unusual spelling there never blocks a write.
            let root = TempRoot::new("known-window");
            let path = root.global();
            std::fs::write(
                &path,
                br#"{"window_config":{"width":1365.3333740234375,"height":767.99999999999999999999}}"#,
            )
            .unwrap();
            GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(true)).unwrap();
            GlobalSettings::update_window_config(&path, &window()).unwrap();
            assert_eq!(raw(&path)["window_config"]["height"], 768.0);
        }

        #[test]
        fn faults_before_the_rename_leave_prior_bytes_or_absence() {
            for existing in [false, true] {
                for stage in [
                    Stage::Locked,
                    Stage::Create,
                    Stage::Write,
                    Stage::Flush,
                    Stage::FileSync,
                    Stage::Rename,
                ] {
                    let root = TempRoot::new("precommit");
                    let path = root.global();
                    if existing {
                        std::fs::write(&path, br#"{"developer_mode":true}"#).unwrap();
                    }
                    let before = std::fs::read(&path).ok();
                    let hook = install(&path, fail_at(stage));
                    assert!(
                        GlobalSettings::save_preference(
                            &path,
                            PreferenceChange::HideBalances(true)
                        )
                        .is_err(),
                        "{:?}",
                        stage
                    );
                    assert!(GlobalSettings::update_window_config(&path, &window()).is_err());
                    assert!(initialize(&root).is_err());
                    assert_eq!(std::fs::read(&path).ok(), before, "{stage:?}");
                    assert!(temporaries(&root.0).is_empty(), "{:?}", stage);
                    drop(hook);
                    // No marker was published: initialization still has to commit.
                    let receipt = initialize(&root).unwrap();
                    assert!(matches!(receipt.persistence(), Persistence::Committed(_)));
                    assert_eq!(receipt.balances_hidden(), Some(true));
                }
            }
        }

        #[cfg(unix)]
        #[test]
        fn a_failed_directory_sync_reports_a_committed_unconfirmed_save() {
            let root = TempRoot::new("dirsync");
            let path = root.global();
            let _hook = install(&path, fail_at(Stage::DirectorySync));
            let receipt =
                GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(true))
                    .unwrap();
            assert!(matches!(
                receipt.persistence(),
                Persistence::Committed(Durability::Unconfirmed(_))
            ));
            assert_eq!(receipt.balances_hidden(), Some(true));
            // The new bytes are visible: the save was not undone.
            assert_eq!(raw(&path)["hide_balances"], true);
            assert_eq!(raw(&path)["balance_privacy_initialized"], true);
            // Legacy wrappers report the committed save as success.
            GlobalSettings::update_window_config(&path, &window()).unwrap();
            assert!(GlobalSettings::update_preference(
                &path,
                PreferenceChange::Motion(MotionPreference::Reduced)
            )
            .is_ok());
        }

        #[test]
        fn a_failed_lock_release_reports_committed_and_does_not_wedge_the_lock() {
            let root = TempRoot::new("unlock");
            let path = root.global();
            let hook = install(&path, fail_at(Stage::Unlock));
            let receipt =
                GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false))
                    .unwrap();
            assert!(matches!(
                receipt.persistence(),
                Persistence::Committed(Durability::Unconfirmed(_))
            ));
            assert_eq!(raw(&path)["balance_privacy_initialized"], true);
            drop(hook);
            GlobalSettings::update_account_tier(&path, AccountTier::Estate).unwrap();
            assert_eq!(raw(&path)["account_tier"], "estate");
            assert_eq!(raw(&path)["hide_balances"], false);
        }

        #[test]
        fn reads_create_nothing_and_writes_create_only_the_stable_lock() {
            let root = TempRoot::new("read-only");
            let path = root.global();
            GlobalSettings::load_preferences(&path).unwrap();
            GlobalSettings::load_window_config(&path);
            GlobalSettings::load_developer_mode(&path);
            assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
            let missing_root = root.0.join("not-created").join(DEFAULT_FILE_NAME);
            assert_eq!(
                GlobalSettings::load_preferences(&missing_root).unwrap(),
                DevicePreferences::default()
            );
            assert!(!root.0.join("not-created").exists());
            GlobalSettings::update_window_config(&path, &window()).unwrap();
            let mut names: Vec<_> = std::fs::read_dir(&root.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            assert_eq!(names, ["global_settings.json", "global_settings.lock"]);
        }

        #[test]
        fn worldview_defaults_and_round_trips_independently_of_other_preferences() {
            let root = TempRoot::new("worldview");
            let path = root.global();
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap().worldview,
                WorldviewMode::Samurai
            );
            assert!(!path.exists());
            for input in [
                r#"{"theme_mode":"light","language":"es","bitcoin_blake2b_beta":true}"#,
                r#"{"theme_mode":"light","language":"es","bitcoin_blake2b_beta":true,"worldview":"future_mode"}"#,
            ] {
                std::fs::write(&path, input).unwrap();
                let before = GlobalSettings::load_preferences(&path).unwrap();
                assert_eq!(before.worldview, WorldviewMode::Samurai);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), input);

                for (mode, key) in [
                    (WorldviewMode::Suitcoiner, "suitcoiner"),
                    (WorldviewMode::Luke, "luke"),
                    (WorldviewMode::Samurai, "samurai"),
                ] {
                    let expected = DevicePreferences {
                        worldview: mode,
                        ..before
                    };
                    let receipt =
                        GlobalSettings::save_preference(&path, PreferenceChange::Worldview(mode))
                            .unwrap();
                    assert_eq!(receipt.preferences(), expected);
                    assert_eq!(receipt.balances_hidden(), None);
                    assert_eq!(GlobalSettings::load_preferences(&path).unwrap(), expected);
                    assert_eq!(raw(&path)["worldview"], key);
                    assert_eq!(raw(&path)["language"], "es");
                }
            }
        }

        #[test]
        fn display_language_round_trips_without_changing_worldview() {
            let root = TempRoot::new("display-language");
            let path = root.global();
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap().language,
                Language::English
            );

            std::fs::write(
                &path,
                r#"{"language":{"future":true},"worldview":"luke","theme_mode":"light"}"#,
            )
            .unwrap();
            let before = GlobalSettings::load_preferences(&path).unwrap();
            assert_eq!(before.language, Language::English);
            assert_eq!(before.worldview, WorldviewMode::Luke);
            for (language, code) in [(Language::Spanish, "es"), (Language::English, "en")] {
                let receipt =
                    GlobalSettings::save_preference(&path, PreferenceChange::Language(language))
                        .unwrap();
                let expected = DevicePreferences { language, ..before };
                assert_eq!(receipt.preferences(), expected);
                assert_eq!(GlobalSettings::load_preferences(&path).unwrap(), expected);
                assert_eq!(raw(&path)["language"], code);
            }
        }

        #[test]
        fn lossy_malformed_language_does_not_rewrite_settings() {
            let root = TempRoot::new("lossy-language");
            let path = root.global();
            let input = r#"{"language":9007199254740993.0,"worldview":"luke"}"#;
            std::fs::write(&path, input).unwrap();
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap().language,
                Language::English
            );
            assert!(
                GlobalSettings::save_preference(&path, PreferenceChange::DeveloperMode(true))
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), input);
        }

        #[test]
        fn artwork_defaults_and_saves_are_independent_per_worldview() {
            use crate::app::settings::preferences::ArtworkStyle::{Existing, Scene};

            let root = TempRoot::new("worldview-artwork");
            let path = root.global();
            std::fs::write(
                &path,
                r#"{"worldview":"luke","language":"es","artwork":{"future_mode":"scene"}}"#,
            )
            .unwrap();
            let defaults = GlobalSettings::load_preferences(&path).unwrap();
            assert_eq!(defaults.artwork.get(WorldviewMode::Samurai), Existing);
            assert_eq!(defaults.artwork.get(WorldviewMode::Suitcoiner), Scene);
            assert_eq!(defaults.artwork.get(WorldviewMode::Luke), Scene);

            for (mode, style) in [
                (WorldviewMode::Samurai, Scene),
                (WorldviewMode::Suitcoiner, Existing),
            ] {
                let receipt =
                    GlobalSettings::save_preference(&path, PreferenceChange::Artwork(mode, style))
                        .unwrap();
                assert_eq!(receipt.preferences().worldview, WorldviewMode::Luke);
                assert_eq!(receipt.preferences().artwork.get(mode), style);
            }
            let selected = GlobalSettings::save_preference(
                &path,
                PreferenceChange::Worldview(WorldviewMode::Samurai),
            )
            .unwrap()
            .preferences();
            assert_eq!(selected.artwork.get(WorldviewMode::Samurai), Scene);
            assert_eq!(selected.artwork.get(WorldviewMode::Suitcoiner), Existing);
            assert_eq!(selected.artwork.get(WorldviewMode::Luke), Scene);
            assert_eq!(GlobalSettings::load_preferences(&path).unwrap(), selected);
            let value = raw(&path);
            assert_eq!(value["language"], "es");
            assert_eq!(value["artwork"]["future_mode"], "scene");
            let changed_language = GlobalSettings::save_preference(
                &path,
                PreferenceChange::Language(Language::English),
            )
            .unwrap()
            .preferences();
            assert_eq!(
                changed_language,
                DevicePreferences {
                    language: Language::English,
                    ..selected
                }
            );
            assert_eq!(raw(&path)["artwork"]["future_mode"], "scene");
        }

        #[test]
        fn non_hide_saves_do_not_establish_privacy_provenance() {
            let root = TempRoot::new("provenance");
            let path = root.global();
            for change in [
                PreferenceChange::Theme(ThemePreference::Light),
                PreferenceChange::LauncherStyle(LauncherStyle::List),
                PreferenceChange::Motion(MotionPreference::Reduced),
                PreferenceChange::DeveloperMode(true),
                PreferenceChange::BitcoinBlake2bBeta(true),
            ] {
                let receipt = GlobalSettings::save_preference(&path, change).unwrap();
                assert!(!receipt.privacy_established(), "{:?}", change);
                assert_eq!(receipt.balances_hidden(), None, "{change:?}");
            }
            let receipt =
                GlobalSettings::save_preference(&path, PreferenceChange::HideBalances(false))
                    .unwrap();
            assert_eq!(receipt.balances_hidden(), Some(false));
            // Established provenance survives later unrelated saves.
            let receipt =
                GlobalSettings::save_preference(&path, PreferenceChange::DeveloperMode(false))
                    .unwrap();
            assert_eq!(receipt.balances_hidden(), Some(false));
            GlobalSettings::update_window_config(&path, &window()).unwrap();
            assert_eq!(raw(&path)["balance_privacy_initialized"], true);
        }

        #[cfg(unix)]
        #[test]
        fn links_and_non_regular_endpoints_are_refused_without_side_effects() {
            use std::os::unix::fs::symlink;
            let root = TempRoot::new("refusals");
            let target = root.0.join("elsewhere.json");
            std::fs::write(&target, br#"{"developer_mode":true}"#).unwrap();
            let snapshot = |root: &TempRoot| {
                let mut entries: Vec<_> = std::fs::read_dir(&root.0)
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                entries.sort();
                entries
            };
            let refused = |path: &Path| {
                assert!(GlobalSettings::load_preferences(path).is_err());
                assert!(GlobalSettings::update_window_config(path, &window()).is_err());
                assert!(GlobalSettings::save_preference(
                    path,
                    PreferenceChange::HideBalances(true)
                )
                .is_err());
            };

            // Settings file as a link to a valid file, and as a dangling link.
            let linked = root.0.join("linked.json");
            symlink(&target, &linked).unwrap();
            refused(&linked);
            let dangling = root.0.join("dangling.json");
            symlink(root.0.join("missing.json"), &dangling).unwrap();
            refused(&dangling);
            assert!(!root.0.join("missing.json").exists());
            // Settings path as a directory.
            let directory = root.0.join("directory.json");
            std::fs::create_dir(&directory).unwrap();
            refused(&directory);
            // Lock endpoint as a link and as a directory.
            let lock_linked = root.0.join("lock-linked.json");
            std::fs::write(&lock_linked, b"{}").unwrap();
            symlink(&target, root.0.join("lock-linked.lock")).unwrap();
            refused(&lock_linked);
            let lock_directory = root.0.join("lock-directory.json");
            std::fs::write(&lock_directory, b"{}").unwrap();
            std::fs::create_dir(root.0.join("lock-directory.lock")).unwrap();
            assert!(GlobalSettings::update_window_config(&lock_directory, &window()).is_err());
            assert!(GlobalSettings::load_preferences(&lock_directory).is_err());

            let before = snapshot(&root);
            refused(&linked);
            refused(&dangling);
            assert_eq!(
                snapshot(&root),
                before,
                "refusals created or removed entries"
            );
            assert_eq!(
                std::fs::read(&target).unwrap(),
                br#"{"developer_mode":true}"#
            );
            assert_eq!(std::fs::read(&lock_linked).unwrap(), b"{}");
            assert!(std::fs::symlink_metadata(&linked)
                .unwrap()
                .file_type()
                .is_symlink());

            // Unreadable file: an error, never defaults over it.
            if permissions_enforced(&root.0) {
                use std::os::unix::fs::PermissionsExt;
                let unreadable = root.0.join("unreadable.json");
                std::fs::write(&unreadable, br#"{"developer_mode":true}"#).unwrap();
                std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
                    .unwrap();
                refused(&unreadable);
                std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
                assert_eq!(
                    std::fs::read(&unreadable).unwrap(),
                    br#"{"developer_mode":true}"#
                );
            }

            // A user-selected data root may itself be a link.
            let real = TempRoot::new("real-root");
            let linked_root = root.0.join("linked-root");
            symlink(&real.0, &linked_root).unwrap();
            let via_link = linked_root.join(DEFAULT_FILE_NAME);
            GlobalSettings::update_window_config(&via_link, &window()).unwrap();
            assert!(real.global().is_file());
        }

        #[test]
        fn a_read_only_settings_file_is_refused_not_replaced() {
            let root = TempRoot::new("read-only-file");
            let path = root.global();
            std::fs::write(&path, br#"{"developer_mode":true}"#).unwrap();
            let original = std::fs::metadata(&path).unwrap().permissions();
            let mut read_only = original.clone();
            read_only.set_readonly(true);
            std::fs::set_permissions(&path, read_only).unwrap();
            assert!(GlobalSettings::update_window_config(&path, &window()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), br#"{"developer_mode":true}"#);
            assert!(temporaries(&root.0).is_empty());
            std::fs::set_permissions(&path, original).unwrap();
        }

        /// The child half runs in another process, so the OS lock (not an
        /// in-process mutex) must keep the parent's read after its commit.
        #[test]
        fn the_global_lock_serializes_writers_across_processes() {
            const CHILD: &str = "COINCUBE_R04B_GLOBAL_LOCK_CHILD";
            if let Some(directory) = std::env::var_os(CHILD) {
                let directory = PathBuf::from(directory);
                let path = directory.join(DEFAULT_FILE_NAME);
                let (locked, waiting) = (
                    directory.join("child-locked"),
                    directory.join("parent-waiting"),
                );
                let _hook = install(&path, move |stage| {
                    if stage == Stage::Locked {
                        std::fs::write(&locked, b"")?;
                        let deadline = Instant::now() + DEADLOCK;
                        while !waiting.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    Ok(())
                });
                GlobalSettings::update_account_tier(&path, AccountTier::Pro).unwrap();
                return;
            }
            let root = TempRoot::new("process-lock");
            let path = root.global();
            let log = root.0.join("child.log");
            // Declared after `root`, so a failing parent kills and reaps it first.
            let child = ChildGuard::spawn(
                &this_test("the_global_lock_serializes_writers_across_processes"),
                &[(CHILD, root.0.as_os_str())],
                &log,
            );
            let locked = root.0.join("child-locked");
            let deadline = Instant::now() + DEADLOCK;
            while !locked.exists() {
                assert!(
                    Instant::now() < deadline,
                    "{}: the child never took the lock",
                    HARNESS
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            let waiting = root.0.join("parent-waiting");
            let observed = path.clone();
            let _hook = install(&path, move |stage| match stage {
                Stage::BeforeLock => std::fs::write(&waiting, b""),
                Stage::Locked => {
                    let committed = std::fs::read(&observed)
                        .map(|bytes| String::from_utf8_lossy(&bytes).contains("\"pro\""))
                        .unwrap_or(false);
                    if committed {
                        Ok(())
                    } else {
                        Err(std::io::Error::other(
                            "entered the transaction before the other process committed",
                        ))
                    }
                }
                _ => Ok(()),
            });
            let parent_path = path.clone();
            worker(move || GlobalSettings::update_window_config(&parent_path, &window()))
                .join()
                .unwrap();
            let status = child
                .wait(DEADLOCK)
                .expect("the child did not exit in time; it was killed and reaped");
            assert!(
                status.success(),
                "{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
            let saved = raw(&path);
            assert_eq!(saved["account_tier"], "pro");
            assert_eq!(saved["window_config"]["width"], 1024.0);
        }

        // --- Privacy initialization decision table ---------------------------

        #[test]
        fn initialization_decision_table() {
            // Empty root: no new-installation proof, so hidden once.
            let root = TempRoot::new("empty");
            assert_eq!(hidden(&root), Some(true));
            assert_eq!(raw(&root.global())["balance_privacy_initialized"], true);

            // Absent global file + a legacy masked Cube.
            let root = TempRoot::new("legacy-masked");
            let legacy_bytes = legacy(&root, ChainId::Bitcoin, &[("a", false), ("b", true)]);
            let receipt = initialize(&root).unwrap();
            assert_eq!(receipt.balances_hidden(), Some(true));
            assert!(matches!(receipt.persistence(), Persistence::Committed(_)));
            assert_eq!(
                std::fs::read(root.chain(ChainId::Bitcoin).join(SETTINGS_FILE_NAME)).unwrap(),
                legacy_bytes
            );

            // Intermediate R04a file: false written, no marker, legacy true.
            let root = TempRoot::new("intermediate-false");
            std::fs::write(
                root.global(),
                br#"{"hide_balances":false,"developer_mode":true,"future":1}"#,
            )
            .unwrap();
            legacy(&root, ChainId::Testnet4, &[("t", true)]);
            assert_eq!(hidden(&root), Some(true));
            let saved = raw(&root.global());
            assert_eq!(saved["developer_mode"], true);
            assert_eq!(saved["future"], 1);

            // Legacy JSON without the new fields, unmasked Cubes only: hidden
            // once on upgrade (A4), not the old automatic false.
            let root = TempRoot::new("legacy-clear");
            std::fs::write(root.global(), br#"{"theme_mode":"light"}"#).unwrap();
            let legacy_bytes = legacy(&root, ChainId::Bitcoin, &[("a", false)]);
            assert_eq!(hidden(&root), Some(true));
            assert_eq!(raw(&root.global())["theme_mode"], "light");
            assert_eq!(
                std::fs::read(root.chain(ChainId::Bitcoin).join(SETTINGS_FILE_NAME)).unwrap(),
                legacy_bytes
            );

            // Unresolved global false + every legacy mask false: still hidden.
            let root = TempRoot::new("global-false-clear");
            std::fs::write(root.global(), br#"{"hide_balances":false}"#).unwrap();
            for chain in ChainId::ALL {
                legacy(&root, chain, &[("a", false)]);
            }
            assert_eq!(hidden(&root), Some(true));

            // Unresolved global true stays true.
            let root = TempRoot::new("global-true");
            std::fs::write(root.global(), br#"{"hide_balances":true}"#).unwrap();
            legacy(&root, ChainId::Bitcoin, &[("a", false)]);
            assert_eq!(hidden(&root), Some(true));

            // An established choice always wins over legacy masks, restart
            // after restart, and is never rewritten by initialization.
            for (established, mask) in [(false, true), (true, false)] {
                let root = TempRoot::new("established");
                legacy(&root, ChainId::Bitcoin, &[("a", mask)]);
                GlobalSettings::save_preference(
                    &root.global(),
                    PreferenceChange::HideBalances(established),
                )
                .unwrap();
                let before = std::fs::read(root.global()).unwrap();
                for _ in 0..2 {
                    let receipt = initialize(&root).unwrap();
                    assert_eq!(receipt.balances_hidden(), Some(established));
                    assert_eq!(receipt.persistence(), &Persistence::Unchanged);
                }
                assert_eq!(std::fs::read(root.global()).unwrap(), before);
            }

            // After the one-time hide, an explicit Show persists and legacy
            // masks are never applied again.
            let root = TempRoot::new("show-after-upgrade");
            legacy(&root, ChainId::Signet, &[("s", true)]);
            assert_eq!(hidden(&root), Some(true));
            let shown = GlobalSettings::save_preference(
                &root.global(),
                PreferenceChange::HideBalances(false),
            )
            .unwrap();
            assert_eq!(shown.balances_hidden(), Some(false));
            assert!(matches!(shown.persistence(), Persistence::Committed(_)));
            legacy(&root, ChainId::Testnet, &[("t", true)]);
            for _ in 0..2 {
                let receipt = initialize(&root).unwrap();
                assert_eq!(receipt.balances_hidden(), Some(false));
                assert_eq!(receipt.persistence(), &Persistence::Unchanged);
            }
            // And an explicit Hide persists the same way.
            GlobalSettings::save_preference(&root.global(), PreferenceChange::HideBalances(true))
                .unwrap();
            assert_eq!(hidden(&root), Some(true));
        }

        #[test]
        fn automatic_initialization_never_shows_amounts_without_new_install_proof() {
            // Every unresolved root hides, including roots that look new or
            // freshly wiped.
            type RootCase = (&'static str, fn(&TempRoot));
            let roots: [RootCase; 5] = [
                ("no-chains", |_: &TempRoot| {}),
                ("scaffolding-only", |root: &TempRoot| {
                    for chain in ChainId::ALL {
                        let directory = root.chain(chain);
                        std::fs::create_dir_all(directory.join("data")).unwrap();
                        std::fs::write(directory.join("settings.lock"), b"").unwrap();
                    }
                }),
                ("empty-metadata", |root: &TempRoot| {
                    let directory = root.chain(ChainId::Bitcoin);
                    std::fs::create_dir_all(&directory).unwrap();
                    std::fs::write(directory.join(SETTINGS_FILE_NAME), br#"{"cubes":[]}"#).unwrap();
                }),
                ("unmasked-everywhere", |root: &TempRoot| {
                    for chain in ChainId::ALL {
                        legacy(root, chain, &[("a", false), ("b", false)]);
                    }
                }),
                ("unrelated-global-writes", |root: &TempRoot| {
                    GlobalSettings::update_window_config(&root.global(), &window()).unwrap();
                    GlobalSettings::save_preference(
                        &root.global(),
                        PreferenceChange::Motion(MotionPreference::Reduced),
                    )
                    .unwrap();
                }),
            ];
            for (tag, setup) in roots {
                let root = TempRoot::new(tag);
                setup(&root);
                let receipt = initialize(&root).unwrap();
                assert_eq!(receipt.balances_hidden(), Some(true), "{tag}");
                assert!(receipt.preferences().hide_balances, "{}", tag);
                assert!(
                    matches!(receipt.persistence(), Persistence::Committed(_)),
                    "{}",
                    tag
                );
                assert_eq!(raw(&root.global())["hide_balances"], true, "{tag}");
            }

            // A missing root is refused and never created.
            let root = TempRoot::new("absent-parent");
            let missing = CoincubeDirectory::new(root.0.join("absent-root"));
            assert!(matches!(
                GlobalSettings::load_or_initialize_preferences(&missing),
                Err(PrivacyInitError::Settings(_))
            ));
            assert!(!root.0.join("absent-root").exists());
        }

        #[test]
        fn unrelated_writes_before_initialization_keep_privacy_unresolved() {
            let root = TempRoot::new("unrelated-first");
            // No legacy mask anywhere: the serialized false is still not authority.
            legacy(&root, ChainId::Signet, &[("s", false)]);
            GlobalSettings::update_window_config(&root.global(), &window()).unwrap();
            GlobalSettings::dismiss_hw_advisory(&root.global(), "x:1".into()).unwrap();
            GlobalSettings::save_preference(
                &root.global(),
                PreferenceChange::Theme(ThemePreference::Dark),
            )
            .unwrap();
            assert_eq!(raw(&root.global())["hide_balances"], false);
            assert_eq!(raw(&root.global())["balance_privacy_initialized"], false);
            assert_eq!(hidden(&root), Some(true));
            // Idempotent and unchanged afterwards.
            assert_eq!(
                initialize(&root).unwrap().persistence(),
                &Persistence::Unchanged
            );
            GlobalSettings::update_window_config(&root.global(), &window()).unwrap();
            assert_eq!(hidden(&root), Some(true));
        }

        #[test]
        fn an_explicit_choice_before_initialization_takes_the_lock_wins() {
            for explicit in [false, true] {
                for global_exists in [false, true] {
                    let root = TempRoot::new("explicit-wins");
                    let path = root.global();
                    if global_exists {
                        std::fs::write(&path, br#"{"account_tier":"pro"}"#).unwrap();
                    }
                    let (mut pause, hook) = pause_once(Stage::BeforeLock);
                    let _hook = install(&path, hook);
                    let init_root = root.root();
                    let initializer =
                        worker(move || GlobalSettings::load_or_initialize_preferences(&init_root));
                    pause.arrived(&initializer); // read an unresolved file, lock not yet taken
                    let choice_path = path.clone();
                    let chosen = worker(move || {
                        GlobalSettings::save_preference(
                            &choice_path,
                            PreferenceChange::HideBalances(explicit),
                        )
                    })
                    .join()
                    .unwrap();
                    assert_eq!(chosen.balances_hidden(), Some(explicit));
                    let window_path = path.clone();
                    worker(move || GlobalSettings::update_window_config(&window_path, &window()))
                        .join()
                        .unwrap();
                    pause.release();
                    let receipt = initializer.join().unwrap();
                    assert_eq!(receipt.balances_hidden(), Some(explicit));
                    assert_eq!(receipt.persistence(), &Persistence::Unchanged);
                    let saved = raw(&path);
                    assert_eq!(saved["hide_balances"], explicit);
                    assert_eq!(saved["window_config"]["width"], 1024.0);
                    if global_exists {
                        assert_eq!(saved["account_tier"], "pro");
                    }
                }
            }
        }

        #[test]
        fn an_explicit_choice_waiting_behind_initialization_commits_after_it() {
            for explicit in [false, true] {
                let root = TempRoot::new("explicit-after");
                let path = root.global();
                let (mut pause, pause_hook) = pause_once(Stage::Rename);
                let (attempts, attempt_hook) = arrivals(Stage::BeforeLock);
                let _hook = install(&path, move |stage| {
                    attempt_hook(stage)?;
                    pause_hook(stage)
                });
                let init_root = root.root();
                let initializer =
                    worker(move || GlobalSettings::load_or_initialize_preferences(&init_root));
                next_arrival(&attempts, "initialization");
                pause.arrived(&initializer); // holds the global lock with true staged
                let choice_path = path.clone();
                let choice = worker(move || {
                    GlobalSettings::save_preference(
                        &choice_path,
                        PreferenceChange::HideBalances(explicit),
                    )
                });
                next_arrival(&attempts, "the explicit choice");
                std::thread::sleep(BLOCKED);
                choice.assert_blocked("the explicit choice");
                pause.release();
                let initialized = initializer.join().unwrap();
                assert_eq!(initialized.balances_hidden(), Some(true));
                assert!(matches!(
                    initialized.persistence(),
                    Persistence::Committed(_)
                ));
                let chosen = choice.join().unwrap();
                assert_eq!(chosen.balances_hidden(), Some(explicit));
                assert_eq!(hidden(&root), Some(explicit));
            }
        }

        #[test]
        fn an_unrelated_writer_before_initialization_takes_the_lock_is_kept() {
            let root = TempRoot::new("unrelated-during");
            let path = root.global();
            let (mut pause, hook) = pause_once(Stage::BeforeLock);
            let _hook = install(&path, hook);
            let init_root = root.root();
            let initializer =
                worker(move || GlobalSettings::load_or_initialize_preferences(&init_root));
            pause.arrived(&initializer);
            let tier_path = path.clone();
            worker(move || GlobalSettings::update_account_tier(&tier_path, AccountTier::Estate))
                .join()
                .unwrap();
            pause.release();
            let receipt = initializer.join().unwrap();
            assert_eq!(receipt.balances_hidden(), Some(true));
            assert!(matches!(receipt.persistence(), Persistence::Committed(_)));
            assert_eq!(raw(&path)["account_tier"], "estate");
        }

        #[test]
        fn orphan_temporaries_are_never_authority() {
            let root = TempRoot::new("orphans");
            let orphan = root
                .0
                .join(format!(".{DEFAULT_FILE_NAME}.{}.tmp", uuid::Uuid::new_v4()));
            // An orphaned established Show must not count as a choice.
            std::fs::write(
                &orphan,
                br#"{"hide_balances":false,"balance_privacy_initialized":true}"#,
            )
            .unwrap();
            let chain = root.chain(ChainId::Testnet);
            std::fs::create_dir_all(chain.join("data")).unwrap();
            std::fs::write(chain.join("settings.lock"), b"").unwrap();
            // Cube files play no part, orphaned or not.
            let chain_orphan = chain.join(format!(".settings-{}.tmp", uuid::Uuid::new_v4()));
            std::fs::write(&chain_orphan, b"{").unwrap();
            let receipt = initialize(&root).unwrap();
            assert_eq!(receipt.balances_hidden(), Some(true));
            assert!(matches!(receipt.persistence(), Persistence::Committed(_)));
            assert!(orphan.is_file() && chain_orphan.is_file());
        }

        // --- Independence from Cube metadata ------------------------------

        #[test]
        fn initialization_never_reads_or_changes_cube_metadata() {
            let root = TempRoot::new("metadata-independent");
            let broken: [&[u8]; 4] = [
                b"{",
                b"[]",
                br#"{"cubes":[{"id":"a"}]}"#,
                br#"{"cubes":[{"id":"x","name":"n","network":"signet","created_at":1,"balance_masked":"yes"}]}"#,
            ];
            // Broken, inconsistent or misplaced metadata on every known chain,
            // opaque seed and wallet files, and layouts no chain owns.
            let mut files = vec![(root.0.join(SETTINGS_FILE_NAME), b"{}".to_vec())];
            for (chain, document) in ChainId::ALL.iter().zip(broken.iter().cycle()) {
                let directory = root.chain(*chain);
                files.push((directory.join(SETTINGS_FILE_NAME), document.to_vec()));
                files.push((
                    directory.join("mnemonics").join("slot-file"),
                    b"seed".to_vec(),
                ));
                files.push((
                    directory.join("data").join("wallet.sqlite"),
                    b"wallet".to_vec(),
                ));
            }
            files.push((
                root.0.join("old-layout").join(SETTINGS_FILE_NAME),
                b"{".to_vec(),
            ));
            for (path, bytes) in &files {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            let listing = |directory: &Path| {
                let mut names: Vec<String> = std::fs::read_dir(directory)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            };
            let before = listing(&root.0);
            // Unreadable Cube directories cannot matter to a global-only decision.
            #[cfg(unix)]
            let unreadable = [
                root.chain(ChainId::Bitcoin),
                root.chain(ChainId::Signet).join("mnemonics"),
            ];
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for directory in &unreadable {
                    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                }
            }

            let receipt = initialize(&root).unwrap();
            assert_eq!(receipt.balances_hidden(), Some(true));
            assert!(matches!(receipt.persistence(), Persistence::Committed(_)));
            let shown = GlobalSettings::save_preference(
                &root.global(),
                PreferenceChange::HideBalances(false),
            )
            .unwrap();
            assert_eq!(shown.balances_hidden(), Some(false));
            assert_eq!(initialize(&root).unwrap().balances_hidden(), Some(false));

            #[cfg(unix)]
            {
                for directory in &unreadable {
                    restore_permissions(directory);
                }
            }
            for (path, bytes) in &files {
                assert_eq!(&std::fs::read(path).unwrap(), bytes, "{path:?} changed");
            }
            // Only the global file and its stable lock are new: no inventory
            // gate, fence or epoch file.
            let mut expected = before;
            expected.extend([
                DEFAULT_FILE_NAME.to_string(),
                "global_settings.lock".to_string(),
            ]);
            expected.sort();
            assert_eq!(listing(&root.0), expected);
        }

        // --- The harness itself -------------------------------------------

        /// An early worker failure, a lock that is never released and a child
        /// that never exits each end within their limits; the stuck child is
        /// killed and reaped rather than left behind.
        #[test]
        fn the_test_harness_fails_within_its_limits_instead_of_hanging() {
            const PROBE: &str = "COINCUBE_R04B_HARNESS_PROBE";
            const PROBE_DIR: &str = "COINCUBE_R04B_HARNESS_PROBE_DIR";
            const LIMIT: Duration = Duration::from_millis(500);
            const NAME: &str = "the_test_harness_fails_within_its_limits_instead_of_hanging";
            if let (Ok(probe), Some(directory)) =
                (std::env::var(PROBE), std::env::var_os(PROBE_DIR))
            {
                let path = PathBuf::from(directory).join(DEFAULT_FILE_NAME);
                match probe.as_str() {
                    "early-failure" => {
                        // Malformed, so the writer fails before reaching the pause.
                        std::fs::write(&path, b"{").unwrap();
                        let (pause, hook) = pause_within(Stage::Rename, LIMIT);
                        let _hook = install(&path, hook);
                        let writer = worker_within(LIMIT, move || {
                            GlobalSettings::update_window_config(&path, &window())
                        });
                        pause.arrived(&writer);
                    }
                    "stuck-lock" | "stuck-child" => {
                        // A second open file description, so its lock excludes
                        // the transaction's even within this process.
                        let lock_path = GlobalSettings::lock_path(&path).unwrap();
                        let lock = open_or_create_lock(&lock_path).unwrap();
                        FileExt::lock_exclusive(&lock).unwrap();
                        std::mem::forget(lock);
                        if probe == "stuck-lock" {
                            worker_within(LIMIT, move || {
                                GlobalSettings::update_window_config(&path, &window())
                            })
                            .join()
                            .unwrap();
                        } else {
                            // Deliberately unbounded: only the parent's watchdog ends it.
                            GlobalSettings::update_window_config(&path, &window()).unwrap();
                        }
                    }
                    other => panic!("unknown probe {}", other),
                }
                panic!("probe {} finished instead of failing", probe);
            }

            let root = TempRoot::new("harness-probes");
            let spawn = |probe: &str| {
                let directory = root.0.join(probe);
                std::fs::create_dir_all(&directory).unwrap();
                let log = root.0.join(format!("{probe}.log"));
                let child = ChildGuard::spawn(
                    &this_test(NAME),
                    &[
                        (PROBE, OsStr::new(probe)),
                        (PROBE_DIR, directory.as_os_str()),
                    ],
                    &log,
                );
                (child, log)
            };
            for (probe, expected) in [
                ("early-failure", "worker finished before reaching the pause"),
                ("stuck-lock", "worker did not finish within"),
            ] {
                let (child, log) = spawn(probe);
                let status = child
                    .wait(DEADLOCK)
                    .expect("a probe child did not exit by itself and was killed");
                let output = std::fs::read_to_string(&log).unwrap_or_default();
                assert!(!status.success(), "{}: {}", probe, output);
                assert!(
                    output.contains(&format!("{HARNESS}: {expected}")),
                    "{}: {}",
                    probe,
                    output
                );
            }
            let (child, log) = spawn("stuck-child");
            let killed = child
                .wait(Duration::from_secs(5))
                .expect_err("a child blocked on a lock exited by itself");
            let output = std::fs::read_to_string(&log).unwrap_or_default();
            assert!(!killed.success(), "{}", output);
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                // Killed by the watchdog, and reaped: the status came from wait().
                assert_eq!(killed.signal(), Some(9), "{output}");
            }
        }

        #[test]
        fn settings_errors_stay_settings_errors() {
            let root = TempRoot::new("error-kind");
            std::fs::write(root.global(), b"{").unwrap();
            assert!(matches!(
                initialize(&root),
                Err(PrivacyInitError::Settings(SettingsError::ReadingFile(_)))
            ));
        }
    }
}

#[cfg(test)]
mod test {
    use super::global::{GlobalSettings, WindowConfig};
    use super::preferences::{
        DevicePreferences, LauncherStyle, MotionPreference, PreferenceChange, ThemePreference,
    };
    use coincube_ui::theme::palette::ThemeMode;
    use std::env;

    use super::WalletSettings;

    /// The marker a restored Vault carries until its rescan has been started.
    ///
    /// Two properties matter and neither is obvious from the type. It has to
    /// survive a write/read cycle, because an interrupted multi-hour scan is
    /// re-offered from the file and nowhere else. And it has to be *absent*
    /// from the file when unset, so the settings of every Cube that never
    /// restored anything are untouched by the field existing.
    #[test]
    fn a_pending_rescan_round_trips_and_stays_out_of_the_file_when_unset() {
        let raw = r#"{
            "name": "Coincube-kt6ht0kt",
            "alias": null,
            "descriptor_checksum": "kt6ht0kt",
            "pinned_at": null,
            "remote_backend_auth": null,
            "start_internal_bitcoind": true,
            "pending_rescan": {"from": 1784953848}
        }"#;
        let parsed: WalletSettings = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed.pending_rescan,
            Some(super::PendingRescan::From(1_784_953_848))
        );
        assert_eq!(
            parsed.pending_rescan.unwrap().timestamp(),
            Some(1_784_953_848)
        );

        let reserialised = serde_json::to_string(&parsed).unwrap();
        assert!(reserialised.contains("\"pending_rescan\":{\"from\":1784953848}"));

        // A Recovery-Kit restore knows a rescan is owed but not from when: the
        // kit records no birthday. It must round-trip as its own state, not
        // collapse to "nothing owed".
        let unknown = raw.replace("{\"from\": 1784953848}", "\"date_unknown\"");
        let parsed: WalletSettings = serde_json::from_str(&unknown).unwrap();
        assert_eq!(
            parsed.pending_rescan,
            Some(super::PendingRescan::DateUnknown)
        );
        assert_eq!(parsed.pending_rescan.unwrap().timestamp(), None);

        // Absent in the file means absent in the struct — a Vault that never
        // restored is not owed a rescan.
        let without = raw.replace(
            ",\n            \"pending_rescan\": {\"from\": 1784953848}",
            "",
        );
        let parsed: WalletSettings = serde_json::from_str(&without).unwrap();
        assert_eq!(parsed.pending_rescan, None);
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("pending_rescan"));
    }

    const RAW_GLOBAL_SETTINGS: &str = r#"{
          "bitbox": {
            "noise_config": {
              "app_static_privkey": [
                84,
                118,
                69,
                7,
                5,
                246,
                50,
                252,
                79,
                62,
                233,
                118,
                54,
                46,
                247,
                143,
                255,
                152,
                11,
                96,
                7,
                213,
                209,
                42,
                219,
                58,
                237,
                22,
                53,
                221,
                227,
                228
              ],
              "device_static_pubkeys": [
                [
                  252,
                  78,
                  254,
                  112,
                  62,
                  72,
                  220,
                  22,
                  23,
                  147,
                  205,
                  166,
                  248,
                  39,
                  97,
                  46,
                  32,
                  255,
                  132,
                  125,
                  97,
                  142,
                  31,
                  146,
                  44,
                  186,
                  231,
                  1,
                  12,
                  190,
                  105,
                  11
                ]
              ]
            }
          },
          "window_config": {
            "width": 1248.0,
            "height": 688.0
          }
        }"#;

    #[test]
    fn bitcoin_blake2b_beta_defaults_off_and_preserves_other_preferences() {
        assert!(!GlobalSettings::default().bitcoin_blake2b_beta);
        let old: GlobalSettings = serde_json::from_str(RAW_GLOBAL_SETTINGS).unwrap();
        assert!(!old.bitcoin_blake2b_beta);
        let dir = env::temp_dir().join(format!("coincube-global-beta-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        assert!(!GlobalSettings::load_bitcoin_blake2b_beta(&path));
        GlobalSettings::update_bitcoin_blake2b_beta(&path, true).unwrap();
        assert!(path.is_file(), "first preference write must be persisted");
        assert!(GlobalSettings::load_bitcoin_blake2b_beta(&path));
        GlobalSettings::update_developer_mode(&path, true).unwrap();
        GlobalSettings::update_bitcoin_blake2b_beta(&path, false).unwrap();
        assert!(!GlobalSettings::load_bitcoin_blake2b_beta(&path));
        assert!(GlobalSettings::load_developer_mode(&path));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn network_activity_defaults_on_and_automatic_and_each_edit_persists() {
        use super::preferences::NetworkActivitySource;
        let fresh = GlobalSettings::default();
        assert!(fresh.network_activity);
        assert_eq!(
            fresh.network_activity_source,
            NetworkActivitySource::Automatic
        );
        // A file written before these fields existed reads as on, Automatic.
        let old: GlobalSettings = serde_json::from_str(RAW_GLOBAL_SETTINGS).unwrap();
        assert!(old.network_activity);
        assert_eq!(
            old.network_activity_source,
            NetworkActivitySource::Automatic
        );
        let dir = env::temp_dir().join(format!("coincube-activity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        let saved = GlobalSettings::update_preference(
            &path,
            PreferenceChange::NetworkActivitySource(NetworkActivitySource::Connect),
        )
        .unwrap();
        assert!(saved.network_activity);
        assert_eq!(
            saved.network_activity_source,
            NetworkActivitySource::Connect
        );
        GlobalSettings::update_preference(&path, PreferenceChange::NetworkActivity(false)).unwrap();
        let loaded = GlobalSettings::load_preferences(&path).unwrap();
        assert!(!loaded.network_activity);
        assert_eq!(
            loaded.network_activity_source,
            NetworkActivitySource::Connect
        );
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["network_activity"], serde_json::json!(false));
        assert_eq!(raw["network_activity_source"], serde_json::json!("connect"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_preferences_are_system_and_each_first_edit_persists() {
        let dir = env::temp_dir().join(format!("coincube-preferences-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (index, change) in [
            PreferenceChange::Theme(ThemePreference::Dark),
            PreferenceChange::LauncherStyle(LauncherStyle::List),
            PreferenceChange::HideBalances(true),
            PreferenceChange::Motion(MotionPreference::Reduced),
            PreferenceChange::DeveloperMode(true),
            PreferenceChange::BitcoinBlake2bBeta(true),
        ]
        .iter()
        .copied()
        .enumerate()
        {
            let path = dir.join(format!("global-{index}.json"));
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap(),
                DevicePreferences::default()
            );
            assert!(!path.exists(), "a read must not create the file");
            let persisted = GlobalSettings::update_preference(&path, change).unwrap();
            assert!(
                path.is_file(),
                "first edit {:?} must create settings",
                change
            );
            assert_eq!(GlobalSettings::load_preferences(&path).unwrap(), persisted);
        }
        let dark = GlobalSettings::load_preferences(&dir.join("global-0.json")).unwrap();
        assert_eq!(dark.theme, ThemePreference::Dark);
        assert_eq!(
            GlobalSettings::load_theme_mode(&dir.join("global-0.json")),
            ThemeMode::Dark
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_theme_presence_migrates_without_writing_on_read() {
        let dir =
            env::temp_dir().join(format!("coincube-theme-migration-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, json, expected) in [
            ("dark", r#"{"theme_mode":"dark"}"#, ThemePreference::Dark),
            ("light", r#"{"theme_mode":"light"}"#, ThemePreference::Light),
            ("missing", "{}", ThemePreference::Dark),
            (
                "new-wins",
                r#"{"theme_mode":"light","theme_preference":"system"}"#,
                ThemePreference::System,
            ),
        ] {
            let path = dir.join(format!("{name}.json"));
            std::fs::write(&path, json).unwrap();
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap().theme,
                expected
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
            GlobalSettings::update_preference(&path, PreferenceChange::HideBalances(true)).unwrap();
            assert_eq!(
                GlobalSettings::load_preferences(&path).unwrap().theme,
                expected
            );
            let written: GlobalSettings =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(written.theme_preference, Some(expected));
        }
        let light = dir.join("light.json");
        GlobalSettings::update_preference(&light, PreferenceChange::Theme(ThemePreference::System))
            .unwrap();
        assert_eq!(
            GlobalSettings::load_preferences(&light).unwrap().theme,
            ThemePreference::System
        );
        assert_eq!(GlobalSettings::load_theme_mode(&light), ThemeMode::Dark);
        let stored: GlobalSettings =
            serde_json::from_slice(&std::fs::read(&light).unwrap()).unwrap();
        assert_eq!(stored.theme_mode, ThemeMode::Dark);
        GlobalSettings::update_theme_mode(&light, ThemeMode::Light).unwrap();
        assert_eq!(
            GlobalSettings::load_preferences(&light).unwrap().theme,
            ThemePreference::Light
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preference_updates_preserve_unrelated_global_values() {
        let dir = env::temp_dir().join(format!(
            "coincube-preference-preserve-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        let mut old: serde_json::Value = serde_json::from_str(RAW_GLOBAL_SETTINGS).unwrap();
        old["account_tier"] = "pro".into();
        old["theme_mode"] = "light".into();
        old["developer_mode"] = true.into();
        old["bitcoin_blake2b_beta"] = true.into();
        old["recipient_identity_checks"] = false.into();
        old["dismissed_hw_advisories"] = serde_json::json!(["device:notice"]);
        old["seen_advisory_notices"] = serde_json::json!(["incident"]);
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        for change in [
            PreferenceChange::Theme(ThemePreference::System),
            PreferenceChange::LauncherStyle(LauncherStyle::List),
            PreferenceChange::Motion(MotionPreference::Reduced),
            PreferenceChange::HideBalances(true),
            PreferenceChange::DeveloperMode(false),
            PreferenceChange::BitcoinBlake2bBeta(false),
        ] {
            GlobalSettings::update_preference(&path, change).unwrap();
        }
        let saved = GlobalSettings::load_preferences(&path).unwrap();
        assert_eq!(saved.theme, ThemePreference::System);
        assert_eq!(saved.launcher_style, LauncherStyle::List);
        assert_eq!(saved.motion, MotionPreference::Reduced);
        assert!(saved.hide_balances);
        assert!(!saved.developer_mode);
        assert!(!saved.bitcoin_blake2b_beta);
        let saved: GlobalSettings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.bitbox.is_some());
        assert_eq!(
            saved.window_config.unwrap(),
            WindowConfig {
                width: 1248.0,
                height: 688.0
            }
        );
        assert_eq!(saved.account_tier, super::global::AccountTier::Pro);
        assert!(!saved.recipient_identity_checks);
        assert_eq!(saved.dismissed_hw_advisories, ["device:notice"]);
        assert_eq!(saved.seen_advisory_notices, ["incident"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_unknown_and_nonfile_settings_return_errors_without_reset() {
        let dir = env::temp_dir().join(format!(
            "coincube-preference-errors-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        for raw in [
            "{invalid",
            r#"{"theme_preference":"unknown"}"#,
            r#"{"theme_preference":null}"#,
            r#"{"launcher_style":"unknown"}"#,
            r#"{"motion_preference":"unknown"}"#,
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(GlobalSettings::load_preferences(&path).is_err());
            assert!(
                GlobalSettings::update_preference(&path, PreferenceChange::HideBalances(true))
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
        let directory_path = dir.join("directory-instead-of-file");
        std::fs::create_dir(&directory_path).unwrap();
        assert!(GlobalSettings::load_preferences(&directory_path).is_err());
        assert!(GlobalSettings::update_preference(
            &directory_path,
            PreferenceChange::HideBalances(true)
        )
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::write(&path, "{}").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert!(GlobalSettings::load_preferences(&path).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let dangling = dir.join("dangling-link");
            std::os::unix::fs::symlink(dir.join("missing-target"), &dangling).unwrap();
            assert!(GlobalSettings::load_preferences(&dangling).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recipient_identity_checks_default_on_for_new_and_older_settings() {
        assert!(GlobalSettings::default().recipient_identity_checks);
        let old: GlobalSettings = serde_json::from_str(RAW_GLOBAL_SETTINGS).unwrap();
        assert!(old.recipient_identity_checks);
        let minimal: GlobalSettings = serde_json::from_str("{}").unwrap();
        assert!(minimal.recipient_identity_checks);
    }

    #[test]
    fn recipient_identity_checks_disabled_survives_first_write_and_reload() {
        let dir = env::temp_dir().join(format!(
            "coincube-recipient-settings-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        assert!(GlobalSettings::load_recipient_identity_checks(&path));
        assert!(!path.exists(), "loading defaults must not create settings");
        GlobalSettings::update_recipient_identity_checks(&path, false).unwrap();
        assert!(
            path.exists(),
            "the first disabled preference must be written"
        );
        assert!(!GlobalSettings::load_recipient_identity_checks(&path));
        let reloaded: GlobalSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!reloaded.recipient_identity_checks);
        GlobalSettings::update_recipient_identity_checks(&path, true).unwrap();
        assert!(GlobalSettings::load_recipient_identity_checks(&path));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recipient_identity_preference_preserves_existing_global_settings() {
        let dir = env::temp_dir().join(format!(
            "coincube-recipient-migration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        std::fs::write(&path, RAW_GLOBAL_SETTINGS).unwrap();
        GlobalSettings::update_recipient_identity_checks(&path, false).unwrap();
        let reloaded: GlobalSettings =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!reloaded.recipient_identity_checks);
        assert!(reloaded.bitbox.is_some());
        assert_eq!(
            reloaded.window_config.unwrap(),
            WindowConfig {
                width: 1248.0,
                height: 688.0
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_parse_global_config() {
        let _ = serde_json::from_str::<GlobalSettings>(RAW_GLOBAL_SETTINGS).unwrap();
    }

    #[test]
    fn test_update_global_config() {
        // Writes leave a stable lock beside the file, so work on a temporary
        // copy rather than mutating the tracked fixture in place.
        let fixture = env::current_dir()
            .unwrap()
            .join("test_assets")
            .join("global_settings.json");
        let dir = env::temp_dir().join(format!("coincube-global-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("global_settings.json");
        std::fs::copy(&fixture, &path).unwrap();
        let fixture_bytes = std::fs::read(&fixture).unwrap();

        // read global config file
        GlobalSettings::update(
            &path,
            |s| {
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
                assert!(s.bitbox.is_some());
                // this must not be written to the file as write == false
                s.window_config.as_mut().unwrap().height = 0.0;
            },
            false,
        )
        .unwrap();

        // re-read the global config file
        GlobalSettings::update(
            &path,
            |s| {
                // change have not been written
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
            },
            true,
        )
        .unwrap();

        // edit the global config file
        GlobalSettings::update(
            &path,
            |s| {
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
                assert!(s.bitbox.is_some());
                // this must be written to the file as write == true
                s.window_config.as_mut().unwrap().height = 0.0;
            },
            true,
        )
        .unwrap();

        // re-read the global config file
        GlobalSettings::update(
            &path,
            |s| {
                // change have been written
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 0.0
                    }
                );
                s.window_config.as_mut().unwrap().height = 688.0;
            },
            true,
        )
        .unwrap();
        assert_eq!(std::fs::read(&fixture).unwrap(), fixture_bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_cube_settings_alias_backward_compat() {
        use super::CubeSettings;

        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "My Cube",
            "network": "bitcoin",
            "backed_up": false,
            "mfa_done": false,
            "created_at": 0,
            "liquid_wallet_signer_fingerprint": "aabbccdd"
        }"#;

        let cube: CubeSettings = serde_json::from_str(json).expect("alias must deserialise");
        assert_eq!(
            cube.master_signer_fingerprint.map(|f| f.to_string()),
            Some("aabbccdd".to_string()),
            "serde alias should map old field name to master_signer_fingerprint"
        );
    }

    #[test]
    fn connect_state_classifies_three_tiers() {
        use super::{CubeConnectState, CubeSettings};
        use coincube_core::miniscript::bitcoin::Network;

        // Sovereign: not registered with Connect.
        let mut cube = CubeSettings::new("Sovereign".to_string(), Network::Bitcoin);
        cube.remote_synced = false;
        assert_eq!(cube.connect_state(), CubeConnectState::Sovereign);
        assert!(!cube.has_recovery_kit());

        // Registered: synced to Connect, no recovery kit yet.
        cube.remote_synced = true;
        assert_eq!(cube.connect_state(), CubeConnectState::Registered);

        // Backed up: synced AND a recovery-kit descriptor has been pushed.
        cube.recovery_kit_last_backed_up_descriptor_fingerprint = Some("abc123".to_string());
        assert!(cube.has_recovery_kit());
        assert_eq!(cube.connect_state(), CubeConnectState::BackedUp);

        // A recovery kit on a Cube that somehow lost remote_synced still reads
        // as Sovereign — registration is the gating signal.
        cube.remote_synced = false;
        assert_eq!(cube.connect_state(), CubeConnectState::Sovereign);
    }

    #[test]
    fn keychain_only_backup_reads_as_backed_up() {
        use super::{CubeConnectState, CubeSettings};
        use coincube_core::miniscript::bitcoin::Network;

        // A keychain-only backup persists the keychain descriptor slot but not
        // the password slot. `has_recovery_kit` must still report a kit, so the
        // Home card reads "Backed up" in agreement with Settings — not "no
        // recovery kit" (keychain-crk-status-fixes).
        let mut cube = CubeSettings::new("Keychain-only".to_string(), Network::Bitcoin);
        cube.remote_synced = true;
        assert!(cube
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .is_none());
        cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint =
            Some("kc-fp".to_string());
        assert!(cube.has_recovery_kit());
        assert_eq!(cube.connect_state(), CubeConnectState::BackedUp);

        // The keychain slot must survive persistence — the field is
        // `skip_serializing_if = "Option::is_none"` but populated here, so it
        // has to appear in the serialized form and come back verbatim. A future
        // `skip_serializing` (or a rename without a serde alias) would silently
        // drop the fingerprint and regress the Cube to "no recovery kit" on the
        // next launch, so round-trip through the JSON persistence API.
        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        let restored: CubeSettings =
            serde_json::from_str(&json).expect("CubeSettings must deserialize");
        assert_eq!(
            restored
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .as_deref(),
            Some("kc-fp"),
            "keychain fingerprint must persist across a serialize/deserialize round-trip"
        );
        assert!(restored.has_recovery_kit());
        assert_eq!(restored.connect_state(), CubeConnectState::BackedUp);
    }

    /// D1: the Vault's identity survives a settings round-trip. The home cube
    /// list reads this field and nothing else — if it were dropped on reload,
    /// every Cube would silently regress to "Vault configured" with no id.
    #[test]
    fn vault_fingerprint_round_trips_through_settings() {
        use super::{CubeSettings, VaultIdentity, WalletId};
        use coincube_core::miniscript::bitcoin::Network;

        let cube =
            CubeSettings::new("Family".to_string(), Network::Bitcoin).with_vault(VaultIdentity {
                wallet_id: WalletId::new("njhdtwde".to_string(), Some(1_700_000_000)),
                fingerprint: Some("8099ee80".to_string()),
            });
        assert_eq!(cube.vault_fingerprint.as_deref(), Some("8099ee80"));

        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        let restored: CubeSettings =
            serde_json::from_str(&json).expect("CubeSettings must deserialize");
        assert_eq!(
            restored.vault_fingerprint.as_deref(),
            Some("8099ee80"),
            "vault fingerprint must persist across a serialize/deserialize round-trip"
        );
        assert_eq!(
            restored
                .vault_wallet_id
                .as_ref()
                .map(|w| &w.descriptor_checksum),
            Some(&"njhdtwde".to_string()),
            "the storage key is a separate value and must not be overwritten by the identity"
        );
    }

    /// D1 back-compat: a `settings.json` written before `vault_fingerprint`
    /// existed must still deserialize, with the field absent rather than
    /// failing the whole file. Those Cubes are what the D4 backfill converges.
    #[test]
    fn pre_field_settings_deserialize_without_a_vault_fingerprint() {
        use super::CubeSettings;

        let json = r#"{
            "id": "cube-1",
            "name": "Legacy",
            "network": "bitcoin",
            "created_at": 0,
            "vault_wallet_id": {
                "timestamp": 1700000000,
                "descriptor_checksum": "njhdtwde"
            }
        }"#;
        let cube: CubeSettings =
            serde_json::from_str(json).expect("pre-field settings must still deserialize");
        assert!(cube.vault_wallet_id.is_some());
        assert_eq!(
            cube.vault_fingerprint, None,
            "an absent field reads as `not asserted yet`, not as an error"
        );
    }

    /// Tenshu v175 L2: the Spark nickname is optional. A `settings.json`
    /// written before it existed loads with no nickname and writes back
    /// without the key, and a set nickname survives a round-trip.
    #[test]
    fn spark_alias_is_optional_and_round_trips() {
        use super::CubeSettings;

        let json = r#"{
            "id": "cube-1",
            "name": "Family",
            "network": "bitcoin",
            "created_at": 0,
            "vault_fingerprint": "8099ee80"
        }"#;
        let cube: CubeSettings =
            serde_json::from_str(json).expect("pre-field settings must still deserialize");
        assert_eq!(cube.spark_alias, None);
        assert_eq!(cube.name, "Family");
        assert_eq!(cube.vault_fingerprint.as_deref(), Some("8099ee80"));
        let written = serde_json::to_value(&cube).expect("CubeSettings must serialize");
        assert!(
            written.get("spark_alias").is_none(),
            "an unset nickname writes nothing, so the file stays as it was"
        );
        let reread: CubeSettings = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(&reread).unwrap(), written);

        let mut named = cube;
        named.spark_alias = Some("Daily spending".to_string());
        let json = serde_json::to_string(&named).expect("CubeSettings must serialize");
        let restored: CubeSettings =
            serde_json::from_str(&json).expect("CubeSettings must deserialize");
        assert_eq!(restored.spark_alias.as_deref(), Some("Daily spending"));
        assert_eq!(restored.name, "Family", "the Cube name is a separate value");
    }

    fn split_from_record(byte: u8) -> super::SplitFromRecord {
        use coincube_core::miniscript::bitcoin::{
            hashes::{sha256, Hash},
            Txid,
        };
        super::SplitFromRecord {
            descriptor_digest: sha256::Hash::hash(&[byte]),
            completed_height: 900_000 + u64::from(byte),
            step2_txid: Txid::from_byte_array([byte; 32]),
        }
    }

    /// B5 (#568): a completed Split's record survives a settings round-trip
    /// verbatim, through the same JSON the settings file is written with.
    #[test]
    fn split_from_round_trips_through_settings() {
        use super::CubeSettings;
        use crate::chain::ChainId;

        let mut cube = CubeSettings::new_with_raw_id(
            "target-cube".to_string(),
            "Target".to_string(),
            ChainId::BitcoinBlake2b,
        );
        cube.split_from = vec![split_from_record(1), split_from_record(2)];
        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        let restored: CubeSettings =
            serde_json::from_str(&json).expect("CubeSettings must deserialize");
        assert_eq!(
            restored.split_from,
            vec![split_from_record(1), split_from_record(2)],
            "every completed Split must come back exactly as written"
        );
    }

    /// B5 back-compat: a `settings.json` written before `split_from` existed
    /// deserializes with no completed Splits, not as an error.
    #[test]
    fn pre_field_settings_deserialize_without_split_from() {
        use super::CubeSettings;

        let json = r#"{
            "id": "cube-1",
            "name": "Legacy",
            "network": "bitcoin-blake2b",
            "created_at": 0,
            "vault_wallet_id": {
                "timestamp": 1700000000,
                "descriptor_checksum": "njhdtwde"
            }
        }"#;
        let cube: CubeSettings =
            serde_json::from_str(json).expect("pre-field settings must still deserialize");
        assert!(cube.vault_wallet_id.is_some());
        assert!(
            cube.split_from.is_empty(),
            "an absent field reads as `no completed Split`, not as an error"
        );
    }

    /// B5: a Cube with no completed Split serializes exactly as before the
    /// field existed (no `split_from` key), so the files of every Cube that
    /// never split stay byte-identical; a completed Split does write it.
    #[test]
    fn split_from_is_omitted_when_empty() {
        use super::CubeSettings;
        use crate::chain::ChainId;

        let mut cube = CubeSettings::new_with_raw_id(
            "target-cube".to_string(),
            "Target".to_string(),
            ChainId::BitcoinBlake2b,
        );
        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        assert!(
            !json.contains("split_from"),
            "an empty split_from must not be written: {}",
            json
        );
        cube.split_from.push(split_from_record(3));
        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        assert!(json.contains("\"split_from\""), "{}", json);
        assert!(json.contains("\"completed_height\":900003"), "{}", json);
    }

    /// D4: the backfill writes once and is a no-op on a Cube that already
    /// carries the right identity — it runs on every wallet load, so a
    /// settings rewrite per open would be pure churn. It does rewrite when the
    /// descriptor changed underneath, which is a genuinely new identity.
    #[test]
    fn adopting_a_vault_fingerprint_writes_once_then_no_ops() {
        use super::{CubeSettings, VaultIdentity, WalletId};
        use coincube_core::miniscript::bitcoin::Network;

        let mut cube =
            CubeSettings::new("Family".to_string(), Network::Bitcoin).with_vault(VaultIdentity {
                wallet_id: WalletId::new("njhdtwde".to_string(), Some(1)),
                // The pre-backfill state: a Vault with no identity.
                fingerprint: None,
            });

        assert!(
            cube.adopt_vault_fingerprint("8099ee80"),
            "the first open must persist the freshly computed identity"
        );
        assert_eq!(cube.vault_fingerprint.as_deref(), Some("8099ee80"));

        assert!(
            !cube.adopt_vault_fingerprint("8099ee80"),
            "every later open must be a no-op — no settings rewrite"
        );

        assert!(
            cube.adopt_vault_fingerprint("deadbeef"),
            "a changed descriptor is a changed identity and must be re-persisted"
        );
        assert_eq!(cube.vault_fingerprint.as_deref(), Some("deadbeef"));
    }

    /// D1: the two halves of the Vault association are written and cleared
    /// together. A fingerprint outliving the Vault it named would have the home
    /// list advertise an id for a Vault that is gone.
    #[test]
    fn clearing_a_vault_clears_both_halves_of_its_identity() {
        use super::{CubeSettings, VaultIdentity, WalletId};
        use coincube_core::miniscript::bitcoin::Network;

        let mut cube =
            CubeSettings::new("Family".to_string(), Network::Bitcoin).with_vault(VaultIdentity {
                wallet_id: WalletId::new("njhdtwde".to_string(), Some(1)),
                fingerprint: Some("8099ee80".to_string()),
            });
        assert!(cube.vault_wallet_id.is_some() && cube.vault_fingerprint.is_some());

        cube.clear_vault();
        assert_eq!(cube.vault_wallet_id, None);
        assert_eq!(
            cube.vault_fingerprint, None,
            "a stale fingerprint must not survive its Vault"
        );
    }

    #[test]
    fn seed_only_kit_on_a_vault_less_cube_reads_as_backed_up() {
        use super::{CubeConnectState, CubeSettings};
        use coincube_core::miniscript::bitcoin::Network;

        // A Cube with no Vault backs up a seed-only Recovery Kit — there is no
        // Wallet Descriptor to fingerprint, so neither drift slot is written.
        // The kit is real and restorable, so the Home card must agree with the
        // Settings card ("Recovery Kit backed up") rather than reporting
        // "Registered to Connect — no recovery kit".
        let mut cube = CubeSettings::new("Cube only".to_string(), Network::Bitcoin);
        cube.remote_synced = true;
        assert!(cube.vault_wallet_id.is_none());
        cube.recovery_kit_password_backed_up = true;
        assert!(cube.has_recovery_kit());
        assert_eq!(cube.connect_state(), CubeConnectState::BackedUp);

        // The flag has no `skip_serializing_if`, but pin the round-trip anyway:
        // losing it on reload is exactly the regression this test exists for.
        let json = serde_json::to_string(&cube).expect("CubeSettings must serialize");
        let restored: CubeSettings =
            serde_json::from_str(&json).expect("CubeSettings must deserialize");
        assert!(restored.recovery_kit_password_backed_up);
        assert_eq!(restored.connect_state(), CubeConnectState::BackedUp);
    }

    #[test]
    fn pre_flag_settings_still_read_as_backed_up() {
        use super::{CubeConnectState, CubeSettings};

        // Back-compat: a Cube backed up by a build that predates
        // `recovery_kit_password_backed_up` has only the password drift
        // fingerprint in settings.json. `#[serde(default)]` gives the absent
        // flag `false`, so the fingerprint has to keep carrying presence —
        // otherwise the upgrade silently regresses those Cubes to
        // "no recovery kit".
        let json = r#"{
            "id": "cube-1",
            "name": "Legacy",
            "network": "bitcoin",
            "created_at": 0,
            "remote_synced": true,
            "recovery_kit_last_backed_up_descriptor_fingerprint": "abc123"
        }"#;
        let cube: CubeSettings = serde_json::from_str(json).expect("must deserialise");
        assert!(!cube.recovery_kit_password_backed_up);
        assert!(cube.has_recovery_kit());
        assert_eq!(cube.connect_state(), CubeConnectState::BackedUp);
    }

    #[test]
    fn backed_up_flag_is_not_recovery_kit() {
        use super::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        // `backed_up` is the local seed-phrase backup, not the server CRK.
        let mut cube = CubeSettings::new("Seed only".to_string(), Network::Bitcoin);
        cube.backed_up = true;
        assert!(
            !cube.has_recovery_kit(),
            "local seed backup must not be mistaken for a Connect recovery kit"
        );
    }

    /// PLAN-connect-blinding D2 hardening: "no seed here" and "there IS a seed
    /// and it would not open" are different facts and must not both arrive as a
    /// bare `None`. Conflating them is how a Cube becomes a permanent straggler
    /// in the A5 registration-coverage report with no visible cause.
    #[test]
    fn missing_seed_and_unreadable_seed_are_distinguishable() {
        use super::{derive_connect_encryption_pubkey, ConnectEncryptionKey};
        use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
        use coincube_core::miniscript::bitcoin::Network;
        use coincube_core::signer::MasterSigner;
        use std::str::FromStr;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("coincube-encpubkey-{}-{}", std::process::id(), seq));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        const PIN: &str = "1234";
        const CUBE: &str = "cube-a";
        let fp = Fingerprint::from_str("deadbeef").unwrap();

        // (1) Nothing on disk at all → NoSeed. Quiet skip, no error state.
        assert!(
            matches!(
                derive_connect_encryption_pubkey(&root, Network::Testnet, fp, PIN, CUBE),
                ConnectEncryptionKey::NoSeed
            ),
            "an absent seed must be reported as NoSeed, not as a failure"
        );

        // (2) Write a real seed for CUBE, then ask for it under a DIFFERENT
        //     cube id. That is exactly the seed_crypt cube_id-binding
        //     disagreement the hardening exists to catch: the file is present
        //     and the AAD does not match, so it must be Unreadable — loud —
        //     rather than silently skipped.
        let signer = MasterSigner::generate(Network::Testnet).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let real_fp = signer.fingerprint(&secp);
        signer
            // `device_secret: None` writes v2 — the format whose AAD binds the
            // cube id and nothing else, which is what this test probes.
            .store_encrypted(&root, Network::Testnet, &secp, None, PIN, CUBE, None)
            .expect("store seed");

        assert!(
            matches!(
                derive_connect_encryption_pubkey(
                    &root,
                    Network::Testnet,
                    real_fp,
                    PIN,
                    "a-different-cube",
                ),
                ConnectEncryptionKey::Unreadable(_)
            ),
            "a present-but-unopenable seed must be Unreadable, never a quiet skip"
        );

        // (3) The same file, asked for correctly, derives the key — so (2) is
        //     about the binding and not about the file being broken.
        let ok = derive_connect_encryption_pubkey(&root, Network::Testnet, real_fp, PIN, CUBE);
        let ConnectEncryptionKey::Derived(hex) = ok else {
            panic!("the correct cube id must derive the key, got {:?}", ok);
        };
        assert_eq!(hex.len(), 66, "33-byte compressed pubkey as hex");
        assert!(hex.starts_with("02") || hex.starts_with("03"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The regression the merge introduced, pinned.
    ///
    /// A **v3** seed needs the OS-keystore device secret, and `coincube-core`
    /// has no keystore access — so `MasterSigner::from_datadir_by_fingerprint`
    /// returns `DeviceSecretRequired` for *every* v3 file. Deriving straight
    /// from disk therefore fails on the now-default Cube format, which would
    /// have meant no Cube minted after the seed hardening ever registered an
    /// encryption pubkey. The session cache (what the unlock that just ran is
    /// already holding) is the path that works, and preferring it is also what
    /// avoids a second ~831 ms Argon2id pass.
    #[test]
    fn a_v3_cube_derives_from_the_session_and_is_loud_without_it() {
        let _guard = crate::app::session::test_guard();
        use super::{derive_connect_encryption_pubkey, ConnectEncryptionKey};
        use coincube_core::miniscript::bitcoin::Network;
        use coincube_core::signer::MasterSigner;
        use std::sync::atomic::{AtomicU64, Ordering};
        use zeroize::Zeroizing;

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "coincube-encpubkey-v3-{}-{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        const PIN: &str = "1234";
        // Namespaced: `app::session` is process-global, so a shared cube id
        // would let these tests clobber each other under `cargo test`.
        let cube_id = format!("cube-v3-{}-{}", std::process::id(), seq);
        let device_secret: coincube_core::seed_crypt::DeviceSecret = Zeroizing::new([0x5a; 32]);

        let signer = MasterSigner::generate(Network::Testnet).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let fp = signer.fingerprint(&secp);
        signer
            .store_encrypted(
                &root,
                Network::Testnet,
                &secp,
                None,
                PIN,
                &cube_id,
                Some(&device_secret),
            )
            .expect("store v3 seed");

        // No session: the disk path cannot open v3 at all. Must be LOUD, not a
        // quiet skip — a quiet skip here is precisely the phantom straggler.
        crate::app::session::close();
        let without_session =
            derive_connect_encryption_pubkey(&root, Network::Testnet, fp, PIN, &cube_id);
        assert!(
            matches!(without_session, ConnectEncryptionKey::Unreadable(_)),
            "a v3 seed with no session must report Unreadable, got {:?}",
            without_session
        );

        // With the session populated — the real post-unlock state — it derives.
        crate::app::session::store_unlocked_signer(
            &cube_id,
            fp,
            signer.try_clone().expect("clone signer"),
        );
        let with_session =
            derive_connect_encryption_pubkey(&root, Network::Testnet, fp, PIN, &cube_id);
        let ConnectEncryptionKey::Derived(hex) = with_session else {
            panic!(
                "a v3 Cube must derive from the session, got {:?}",
                with_session
            );
        };
        assert_eq!(hex.len(), 66);

        // It must be the key the envelope codec would use for this seed —
        // registering anything else silently strands every sealed envelope.
        let expected =
            crate::services::connect::crypto::CubeEncryptionKey::derive(&signer, Network::Testnet)
                .public_key_hex();
        assert_eq!(hex, expected);

        crate::app::session::close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other half of the matrix: a **v2** seed written before the cube-id
    /// binding existed. `seed_crypt` keeps an unbound-AAD (empty `cube_id`)
    /// fallback for exactly these files, so they must open from disk with no
    /// session at all — the pre-hardening Cube keeps working.
    #[test]
    fn a_pre_hardening_v2_seed_derives_from_disk() {
        let _guard = crate::app::session::test_guard();
        use super::{derive_connect_encryption_pubkey, ConnectEncryptionKey};
        use coincube_core::miniscript::bitcoin::Network;
        use coincube_core::signer::MasterSigner;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "coincube-encpubkey-v2-{}-{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        const PIN: &str = "1234";
        let signer = MasterSigner::generate(Network::Testnet).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let fp = signer.fingerprint(&secp);

        // Written with an EMPTY cube id: a v2 file from before the binding.
        signer
            .store_encrypted(&root, Network::Testnet, &secp, None, PIN, "", None)
            .expect("store pre-hardening v2 seed");

        // Opened while naming a Cube, with no session — the unbound-AAD
        // fallback is what has to carry this, and it is v2-only by design.
        crate::app::session::close();
        let out =
            derive_connect_encryption_pubkey(&root, Network::Testnet, fp, PIN, "cube-minted-later");
        let ConnectEncryptionKey::Derived(hex) = out else {
            panic!("a pre-hardening v2 seed must still derive, got {:?}", out);
        };
        assert_eq!(
            hex,
            crate::services::connect::crypto::CubeEncryptionKey::derive(&signer, Network::Testnet)
                .public_key_hex()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Why the passkey unlock path must **not** use
    /// `derive_connect_encryption_pubkey`.
    ///
    /// The helper resolves the seed itself — session cache first, seed file
    /// second. A passkey Cube has no seed file at all, so if the session lookup
    /// misses (the passkey flow awaits a settings write between parking the
    /// signer and needing the key, so its lookup is a *second* one), the
    /// fallback has nothing to read and the answer is `NoSeed` — which callers
    /// are told to skip quietly.
    ///
    /// Quietly is exactly wrong for that Cube: it would never register an
    /// encryption pubkey, its Contacts could never enrol enveloped keys against
    /// it, and it would sit in the coverage report as an unexplained straggler.
    /// `gui::tab`'s passkey arm therefore derives from the signer it is already
    /// holding. This test exists so that reverting it to the helper fails here.
    #[test]
    fn a_seedless_cube_reports_no_seed_rather_than_deriving() {
        use super::{derive_connect_encryption_pubkey, ConnectEncryptionKey};
        use coincube_core::miniscript::bitcoin::Network;
        use coincube_core::signer::MasterSigner;

        let root = std::env::temp_dir().join(format!(
            "coincube-encpubkey-seedless-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // A fingerprint no session entry can match (this signer was never
        // stored anywhere), against a datadir holding no seed files. That is
        // the shape of a passkey Cube whose session lookup missed — and it
        // needs no `session::close()`, so it cannot perturb the process-global
        // session that sibling tests share.
        let signer = MasterSigner::generate(Network::Testnet).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let fp = signer.fingerprint(&secp);

        let out = derive_connect_encryption_pubkey(&root, Network::Testnet, fp, "", "cube-passkey");
        assert!(
            matches!(out, ConnectEncryptionKey::NoSeed),
            "a seedless Cube must not silently derive something; got {:?}",
            out
        );

        // And the value the passkey path uses instead is the same one every
        // other path would have produced from this seed. Checked the same way
        // `missing_seed_and_unreadable_seed_are_distinguishable` checks its
        // derived key: length alone would pass for any 33-byte blob, so pin the
        // SEC1 compressed-point prefix too — that is what makes it a *public
        // key* rather than 66 hex characters.
        let direct =
            crate::services::connect::crypto::CubeEncryptionKey::derive(&signer, Network::Testnet)
                .public_key_hex();
        assert_eq!(direct.len(), 66, "33-byte compressed pubkey as hex");
        assert!(
            direct.starts_with("02") || direct.starts_with("03"),
            "not a SEC1 compressed point: {}",
            direct
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn from_file_distinguishes_missing_empty_and_valid() {
        use super::{Settings, SettingsError, SETTINGS_FILE_NAME};
        use crate::dir::NetworkDirectory;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("coincube-from-file-{}-{}", std::process::id(), seq));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let nd = NetworkDirectory::new(dir.clone());
        let path = dir.join(SETTINGS_FILE_NAME);

        // Genuinely absent file → NotFound (retries exhaust without changing it).
        assert!(matches!(
            Settings::from_file(&nd),
            Err(SettingsError::NotFound)
        ));

        // An existing-but-empty file is corrupt, not absent: the retry loop must
        // terminate and surface a read error — never NotFound, never hang. (A
        // transient empty read of a real file is what these retries paper over;
        // a persistently-empty one falls through to this error.)
        std::fs::write(&path, b"").unwrap();
        assert!(matches!(
            Settings::from_file(&nd),
            Err(SettingsError::ReadingFile(_))
        ));

        // A well-formed file parses on the first read.
        std::fs::write(&path, br#"{"cubes":[],"wallets":[]}"#).unwrap();
        let settings = Settings::from_file(&nd).expect("valid settings must parse");
        assert!(settings.cubes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod chain_identity_tests {
    //! `CubeSettings.network` as a `ChainId` (BTCB2 plan PR 2, dormant slice):
    //! old files keep parsing, the fork variants persist distinctly, unknown
    //! identifiers are rejected, and a BTCB2 Cube's settings live in — and are
    //! read back from — their own directory.

    use super::*;
    use crate::chain::ChainId;
    use crate::dir::CoincubeDirectory;
    use coincube_core::miniscript::bitcoin::Network;

    fn temp_root(tag: &str) -> CoincubeDirectory {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "coincube-chain-id-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        CoincubeDirectory::new(path)
    }

    /// A `settings.json` exactly as every build before this one wrote it.
    fn legacy_file(network: &str) -> String {
        format!(
            r#"{{
              "cubes": [{{
                "id": "6c5d5f4a-3a53-4f5c-9d0e-0a1b2c3d4e5f",
                "name": "Old Cube",
                "network": "{network}",
                "backed_up": true,
                "created_at": 1720000000
              }}],
              "wallets": []
            }}"#
        )
    }

    #[test]
    fn every_legacy_network_string_still_parses_to_the_same_chain() {
        for network in [
            Network::Bitcoin,
            Network::Testnet,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            let raw = legacy_file(&network.to_string());
            let parsed: Settings = serde_json::from_str(&raw).unwrap();
            let cube = &parsed.cubes[0];
            assert_eq!(cube.network, ChainId::from(network), "{}", network);
            assert_eq!(cube.network.bitcoin_network(), network);
            // …and writes back byte-identically for the network field.
            let out = serde_json::to_string(cube).unwrap();
            assert!(
                out.contains(&format!("\"network\":\"{}\"", network)),
                "{}",
                out
            );
        }
    }

    #[test]
    fn a_blake2b_cube_persists_distinctly_and_an_old_build_refuses_it() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let cube = CubeSettings::new("Fork Cube".to_string(), chain);
            let out = serde_json::to_string(&cube).unwrap();
            let expected = format!("\"network\":\"{}\"", chain.dir_name());
            assert!(out.contains(&expected), "{}", out);
            let back: CubeSettings = serde_json::from_str(&out).unwrap();
            assert_eq!(back.network, chain);
            assert_eq!(back.api_network_string(), chain.api_str());
            assert!(!back.runtime_support().is_supported());

            // What a previous build does with this file: its `network` field is
            // a `bitcoin::Network`, which has no such string. It refuses the
            // record rather than reading it as mainnet.
            #[derive(serde::Deserialize)]
            struct OldCube {
                #[allow(dead_code)]
                network: Network,
            }
            assert!(
                serde_json::from_str::<OldCube>(&out).is_err(),
                "an old build would have accepted {}",
                out
            );
        }
    }

    #[test]
    fn unknown_network_identifiers_are_rejected_not_defaulted() {
        for bad in ["mainnet", "btcb2", "bitcoin_blake2b", "Bitcoin", ""] {
            let raw = legacy_file(bad);
            assert!(
                serde_json::from_str::<Settings>(&raw).is_err(),
                "{:?} was accepted",
                bad
            );
        }
    }

    /// The owner is the account of the Cube's *first* sync, and only that: a
    /// later sync under another account (which re-registers the same UUID as
    /// a new server row) must not take ownership, and a Cube already synced
    /// before owners were tracked must not be attributed to whoever syncs it
    /// next.
    #[tokio::test]
    async fn mark_cube_synced_records_the_owner_once_on_first_sync() {
        let root = temp_root("connect-owner");
        let dir = root.network_directory(ChainId::Bitcoin);
        let owner = |id: u32, email: &str| ConnectOwner {
            user_id: id,
            email: email.to_string(),
        };

        let fresh = CubeSettings::new("fresh".to_string(), ChainId::Bitcoin);
        let mut legacy = CubeSettings::new("legacy".to_string(), ChainId::Bitcoin);
        legacy.remote_synced = true;
        let (fresh_id, legacy_id) = (fresh.id.clone(), legacy.id.clone());
        update_settings_file(&dir, move |mut s| {
            s.cubes.push(fresh);
            s.cubes.push(legacy);
            Some(s)
        })
        .await
        .unwrap();

        mark_cube_synced(&dir, &fresh_id, Some(owner(1, "a@example.com")))
            .await
            .unwrap();
        mark_cube_synced(&dir, &fresh_id, Some(owner(2, "b@example.com")))
            .await
            .unwrap();
        mark_cube_synced(&dir, &legacy_id, Some(owner(2, "b@example.com")))
            .await
            .unwrap();

        let saved = Settings::from_file(&dir).unwrap();
        let find = |id: &str| saved.cubes.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            find(&fresh_id).connect_owner,
            Some(owner(1, "a@example.com"))
        );
        assert_eq!(find(&legacy_id).connect_owner, None);
    }

    #[test]
    fn connect_owner_matches_email_case_insensitively() {
        let owner = ConnectOwner {
            user_id: 1,
            email: "CoinCubeAcc2@gmail.com".to_string(),
        };
        assert!(owner.is_email("coincubeacc2@gmail.com "));
        assert!(!owner.is_email("someone.else@gmail.com"));
    }

    #[tokio::test]
    async fn a_blake2b_cube_is_written_to_and_read_from_its_own_directory() {
        let root = temp_root("btcb2-dir");
        let bitcoin_dir = root.network_directory(ChainId::Bitcoin);
        let btcb2_dir = root.network_directory(ChainId::BitcoinBlake2b);

        // One Cube on each chain, written through the real settings writer.
        let bitcoin_cube = CubeSettings::new("Bitcoin".to_string(), ChainId::Bitcoin);
        let btcb2_cube = CubeSettings::new("Blake2b".to_string(), ChainId::BitcoinBlake2b);
        let b = bitcoin_cube.clone();
        update_settings_file(&bitcoin_dir, move |mut s| {
            s.cubes.push(b);
            Some(s)
        })
        .await
        .unwrap();
        let f = btcb2_cube.clone();
        update_settings_file(&btcb2_dir, move |mut s| {
            s.cubes.push(f);
            Some(s)
        })
        .await
        .unwrap();

        // Each directory holds exactly its own chain's Cube.
        let in_bitcoin = Settings::from_file(&bitcoin_dir).unwrap();
        let in_btcb2 = Settings::from_file(&btcb2_dir).unwrap();
        assert_eq!(in_bitcoin.cubes.len(), 1);
        assert_eq!(in_bitcoin.cubes[0].id, bitcoin_cube.id);
        assert_eq!(in_bitcoin.cubes[0].network, ChainId::Bitcoin);
        assert_eq!(in_btcb2.cubes.len(), 1);
        assert_eq!(in_btcb2.cubes[0].id, btcb2_cube.id);
        assert_eq!(in_btcb2.cubes[0].network, ChainId::BitcoinBlake2b);
        assert!(root
            .path()
            .join("bitcoin-blake2b")
            .join(SETTINGS_FILE_NAME)
            .exists());
        // The Bitcoin file is untouched by the BTCB2 write, byte for byte.
        let bitcoin_raw =
            std::fs::read_to_string(bitcoin_dir.path().join(SETTINGS_FILE_NAME)).unwrap();
        assert!(!bitcoin_raw.contains("Blake2b"));
        assert!(!bitcoin_raw.contains("bitcoin-blake2b"));

        // Directory identity and persisted identity agree — the invariant the
        // open gate checks before any projection to `bitcoin::Network`.
        for (dir, chain) in [
            (&bitcoin_dir, ChainId::Bitcoin),
            (&btcb2_dir, ChainId::BitcoinBlake2b),
        ] {
            for cube in Settings::from_file(dir).unwrap().cubes {
                assert_eq!(cube.network, chain);
            }
        }
        let _ = std::fs::remove_dir_all(root.path());
    }

    #[test]
    fn connect_network_strings_come_from_the_identity() {
        assert_eq!(network_to_api_str(Network::Bitcoin), "mainnet");
        assert_eq!(network_to_api_str(ChainId::Bitcoin), "mainnet");
        assert_eq!(network_to_api_str(ChainId::Testnet4), "testnet4");
        assert_eq!(
            network_to_api_str(ChainId::BitcoinBlake2b),
            "bitcoin-blake2b"
        );
        assert_eq!(
            network_to_api_str(ChainId::BitcoinBlake2bTestnet4),
            "bitcoin-blake2b-testnet4"
        );
        // Never "mainnet" for the fork, however it is spelled.
        assert_ne!(
            CubeSettings::new("x".to_string(), ChainId::BitcoinBlake2b).api_network_string(),
            "mainnet"
        );
    }
}

#[cfg(test)]
mod chain_seed_backfill_tests {
    use super::*;
    use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};

    #[test]
    fn backfill_and_encryption_key_use_only_the_cube_chain() {
        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let root =
                std::env::temp_dir().join(format!("chain-backfill-{}", uuid::Uuid::new_v4()));
            let cube_id = format!("chain-backfill-{}", uuid::Uuid::new_v4());
            let signer = MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let fp = signer.fingerprint(&secp);
            let info = Some((MASTER_SEED_LABEL.to_string(), 1000));
            signer
                .store_encrypted(
                    &root,
                    chain.bitcoin_network(),
                    &secp,
                    info.clone(),
                    "1234",
                    &cube_id,
                    None,
                )
                .unwrap();
            crate::app::session::store_unlocked_signer(&cube_id, fp, signer.try_clone().unwrap());
            assert!(matches!(
                derive_connect_encryption_pubkey(
                    &root,
                    chain.bitcoin_network(),
                    fp,
                    "wrong-pin",
                    &cube_id
                ),
                ConnectEncryptionKey::Derived(_)
            ));
            assert_eq!(
                derive_master_signer_fingerprint(&root, chain, "1234", &cube_id, 1000),
                None
            );
            assert!(!matches!(
                derive_connect_encryption_pubkey(&root, chain, fp, "1234", &cube_id),
                ConnectEncryptionKey::Derived(_)
            ));
            signer
                .store_encrypted_for_chain(&root, chain, &secp, info, "5678", &cube_id, None)
                .unwrap();
            assert_eq!(
                derive_master_signer_fingerprint(&root, chain, "5678", &cube_id, 1000),
                Some(fp)
            );
            assert!(matches!(
                derive_connect_encryption_pubkey(&root, chain, fp, "5678", &cube_id),
                ConnectEncryptionKey::Derived(_)
            ));
            assert!(!matches!(
                derive_connect_encryption_pubkey(&root, chain, fp, "1234", &cube_id),
                ConnectEncryptionKey::Derived(_)
            ));
            crate::app::session::close();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod fork_atomic_save_tests {
    use super::*;

    fn directory() -> NetworkDirectory {
        NetworkDirectory::new(
            std::env::temp_dir()
                .join(format!("fork-atomic-{}", uuid::Uuid::new_v4()))
                .join("bitcoin-blake2b"),
        )
    }

    #[tokio::test]
    async fn cancellation_preserves_prior_file_and_removes_partial_temp() {
        for existing in [false, true] {
            let dir = directory();
            if existing {
                update_settings_file(&dir, Some).await.unwrap();
            }
            let path = dir.path().join(SETTINGS_FILE_NAME);
            let before = std::fs::read(&path).ok();
            let (reached, ready) = tokio::sync::oneshot::channel();
            let paused = async move {
                reached.send(()).unwrap();
                std::future::pending::<()>().await;
            };
            let task_dir = dir.clone();
            let task = tokio::spawn(async move {
                update_fork_settings_file(
                    &task_dir,
                    |mut s| {
                        s.cubes
                            .push(CubeSettings::new("new".into(), ChainId::BitcoinBlake2b));
                        Some(s)
                    },
                    paused,
                )
                .await
            });
            ready.await.unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(std::fs::read(&path).ok(), before);
            assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
            // The cancelled writer released its stable lock; another writer
            // can commit normally without losing an earlier complete file.
            update_settings_file(&dir, |mut s| {
                s.cubes
                    .push(CubeSettings::new("after".into(), ChainId::BitcoinBlake2b));
                Some(s)
            })
            .await
            .unwrap();
            assert_eq!(Settings::from_file(&dir).unwrap().cubes.len(), 1);
            std::fs::remove_dir_all(dir.path().parent().unwrap()).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_fork_writers_read_latest_committed_settings() {
        let dir = directory();
        let (reached, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let first_dir = dir.clone();
        let first = tokio::spawn(async move {
            update_fork_settings_file(
                &first_dir,
                |mut s| {
                    s.cubes
                        .push(CubeSettings::new("first".into(), ChainId::BitcoinBlake2b));
                    Some(s)
                },
                async move {
                    reached.send(()).unwrap();
                    wait.await.unwrap();
                },
            )
            .await
        });
        ready.await.unwrap();
        let second_dir = dir.clone();
        let second = tokio::spawn(async move {
            update_settings_file(&second_dir, |mut s| {
                s.cubes
                    .push(CubeSettings::new("second".into(), ChainId::BitcoinBlake2b));
                Some(s)
            })
            .await
        });
        // Keep the first writer paused while the second has an opportunity
        // to commit. Merely releasing immediately would also pass without
        // the lock because filesystem awaits order the writers by accident.
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        assert!(
            !second.is_finished(),
            "the second fork writer committed while the first held the stable sibling lock"
        );
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        let saved = Settings::from_file(&dir).unwrap();
        assert_eq!(saved.cubes.len(), 2);
        assert_eq!(saved.cubes[0].name, "first");
        assert_eq!(saved.cubes[1].name, "second");
        std::fs::remove_dir_all(dir.path().parent().unwrap()).unwrap();
    }
}

#[cfg(test)]
mod bitcoin_atomic_import_settings_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bitcoin_settings_writers_preserve_concurrent_cube_updates_and_owner_permissions() {
        let root = std::env::temp_dir().join(format!("bitcoin-settings-{}", uuid::Uuid::new_v4()));
        let directory = NetworkDirectory::new(root.join("bitcoin"));
        update_settings_file(&directory, Some).await.unwrap();
        let tasks: Vec<_> = (0..24)
            .map(|index| {
                let directory = directory.clone();
                tokio::spawn(async move {
                    update_settings_file(&directory, |mut settings| {
                        settings
                            .cubes
                            .push(CubeSettings::new(format!("Cube {index}"), ChainId::Bitcoin));
                        Some(settings)
                    })
                    .await
                    .unwrap();
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let saved = Settings::from_file(&directory).unwrap();
        assert_eq!(saved.cubes.len(), 24);
        let names: HashSet<_> = saved.cubes.iter().map(|cube| &cube.name).collect();
        assert_eq!(names.len(), 24);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join(SETTINGS_FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn interrupted_bitcoin_settings_write_preserves_prior_bytes_and_retry() {
        let root =
            std::env::temp_dir().join(format!("bitcoin-settings-cancel-{}", uuid::Uuid::new_v4()));
        let directory = NetworkDirectory::new(root.join("bitcoin"));
        update_settings_file(&directory, Some).await.unwrap();
        let before = std::fs::read(directory.path().join(SETTINGS_FILE_NAME)).unwrap();
        let (reached, ready) = tokio::sync::oneshot::channel();
        let task_dir = directory.clone();
        let task = tokio::spawn(async move {
            update_fork_settings_file(
                &task_dir,
                |mut settings| {
                    let mut cube = CubeSettings::new("Importing".into(), ChainId::Bitcoin);
                    cube.bitcoin_import = Some(Box::new(BitcoinImport::pending()));
                    settings.cubes.push(cube);
                    Some(settings)
                },
                async move {
                    reached.send(()).unwrap();
                    std::future::pending::<()>().await;
                },
            )
            .await
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            std::fs::read(directory.path().join(SETTINGS_FILE_NAME)).unwrap(),
            before
        );
        update_settings_file(&directory, |mut settings| {
            settings
                .cubes
                .push(CubeSettings::new("Retry".into(), ChainId::Bitcoin));
            Some(settings)
        })
        .await
        .unwrap();
        assert_eq!(Settings::from_file(&directory).unwrap().cubes.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
impl PendingSparkShift {
    /// A live USDt-on-Liquid → bitcoin shift, for tests.
    pub fn sample() -> Self {
        let option = crate::services::sideshift::deposit_options()[0];
        Self {
            shift: crate::services::sideshift::ShiftResponse {
                id: "d97d3994cd95e54364b6".to_string(),
                deposit_address: "VJL6BYGXdepositaddressVDVL".to_string(),
                deposit_coin: Some(option.coin.to_string()),
                deposit_network: Some(option.network.network_slug().to_string()),
                settle_address: Some("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg".to_string()),
                settle_coin: Some("BTC".to_string()),
                settle_network: Some("bitcoin".to_string()),
                deposit_min: Some("10".to_string()),
                deposit_max: Some("20000".to_string()),
                deposit_amount: None,
                settle_amount: None,
                rate: None,
                // No expiry: tests that need one set it, so none of them
                // start failing as the calendar passes a fixed date.
                expires_at: None,
                created_at: Some("2026-11-13T00:00:00.000Z".to_string()),
                status: Some("waiting".to_string()),
                affiliate_fee_percent: None,
                network_fee_usd: None,
            },
            deposit_option: option.key(),
            refund_address: "VJL6BYGXrefundaddressVDVL".to_string(),
        }
    }
}

#[cfg(test)]
mod pending_spark_shift_tests {
    //! The stored Spark cross-network receive: absent from files that never had
    //! one, and read back exactly as written.

    use super::*;
    use coincube_core::miniscript::bitcoin::Network;

    #[test]
    fn a_cube_without_a_conversion_writes_nothing_and_old_files_still_load() {
        let cube = CubeSettings::new("Family".to_string(), Network::Bitcoin);
        let json = serde_json::to_value(&cube).unwrap();
        assert!(json.get("pending_spark_shift").is_none());

        let back: CubeSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.pending_spark_shift, None);
    }

    #[test]
    fn a_stored_conversion_round_trips_exactly() {
        let mut cube = CubeSettings::new("Family".to_string(), Network::Bitcoin);
        cube.pending_spark_shift = Some(PendingSparkShift::sample());

        let json = serde_json::to_string(&cube).unwrap();
        let back: CubeSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pending_spark_shift, Some(PendingSparkShift::sample()));
    }
}
