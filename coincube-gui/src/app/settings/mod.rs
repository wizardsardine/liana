//! Settings is the module to handle the GUI settings file.
//! The settings file is used by the GUI to store useful information.
pub mod display;
pub mod fiat;
pub mod unit;

use std::collections::{HashMap, HashSet};

use async_fd_lock::LockWrite;
use coincube_core::descriptors::CoincubeDescriptor;
use std::io::SeekFrom;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_ui::component::form;
use serde::{Deserialize, Serialize};

use crate::{
    backup::{Key, KeyRole, KeyType},
    dir::NetworkDirectory,
    hw::HardwareWalletConfig,
    services::{self, connect::client::backend},
    utils::serde::ok_or_none,
};

use coincube_core::miniscript::bitcoin::Network;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

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

pub async fn update_settings_file<F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Option<Settings>,
{
    let path = network_dir.path().join(SETTINGS_FILE_NAME);

    // Whether settings.json already existed before we touch anything. Only a
    // brand-new file is safe to treat as "no prior settings"; a pre-existing
    // file that reads back empty is corrupt/truncated and must NOT be silently
    // replaced with defaults (that would drop the stored cube configuration),
    // so it falls through to a parse error below. Checked before the create.
    let file_existed = tokio::fs::try_exists(&path).await.unwrap_or(false);

    // Open the settings file, retrying briefly on the transient failures
    // Windows surfaces for freshly-created files. OpenOptions creates the file
    // but never its parent directories, so we (re)create the network dir each
    // attempt; and on Windows a virus scanner or search indexer momentarily
    // holds a new file (or its just-created parent), so create(true) can return
    // ERROR_PATH_NOT_FOUND ("os error 3") or a sharing/permission error even
    // though the path is valid. Both map to NotFound / PermissionDenied — retry
    // those with a short backoff; surface anything else immediately. This also
    // hardens the real app, where AV software locks settings.json in the same way.
    let raw_file = {
        let mut attempt = 0u32;
        loop {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    SettingsError::WritingFile(format!("Creating settings dir: {}", e))
                })?;
            }
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .await
            {
                Ok(f) => break f,
                Err(e)
                    if attempt < 5
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                        ) =>
                {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)))
                        .await;
                }
                Err(e) => return Err(SettingsError::ReadingFile(format!("Opening file: {}", e))),
            }
        }
    };

    let mut file = raw_file
        .lock_write()
        .await
        .map_err(|e| SettingsError::ReadingFile(format!("Locking file: {:?}", e)))?;

    // A file we created in this call starts empty and means "no prior
    // settings". A file that already existed is parsed — including when it reads
    // back empty, which surfaces as a parse error rather than silently
    // discarding the previous contents.
    let settings = if file_existed {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| SettingsError::ReadingFile(format!("Reading file content: {}", e)))?;
        serde_json::from_slice::<Settings>(&file_content)
            .map_err(|e| SettingsError::ReadingFile(e.to_string()))?
    } else {
        Settings::default()
    };

    let settings = updater(settings);

    // If updater returns None, delete the file. Drop the locked handle first so
    // the file isn't held open when we unlink it (required on Windows).
    let Some(settings) = settings else {
        drop(file);
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| SettingsError::DeletingFile(e.to_string()))?;
        return Ok(());
    };

    let content = serde_json::to_vec_pretty(&settings)
        .map_err(|e| SettingsError::WritingFile(format!("Failed to serialize settings: {}", e)))?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        SettingsError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        SettingsError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| SettingsError::WritingFile(format!("Failed to truncate file: {}", e)))?;

    // Flush and fsync so the bytes are durably on disk, then drop the handle to
    // release the lock — all before returning. Without this a subsequent reader
    // (Settings::from_file takes no lock) can observe stale/empty content on
    // Windows, where dropping an async handle does not guarantee the write has
    // landed yet.
    file.flush()
        .await
        .map_err(|e| SettingsError::WritingFile(format!("Failed to flush settings: {}", e)))?;
    file.inner_mut()
        .sync_all()
        .await
        .map_err(|e| SettingsError::WritingFile(format!("Failed to sync settings: {}", e)))?;
    drop(file);

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
pub async fn mark_cube_synced(
    network_dir: &NetworkDirectory,
    cube_id: &str,
) -> Result<(), SettingsError> {
    update_settings_file(network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
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
    network: Network,
    pin: &str,
    cube_id: &str,
    cube_created_at: i64,
) -> Option<Fingerprint> {
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
        .join(network.to_string())
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
        MasterSigner::from_datadir_by_fingerprint(datadir_root, network, fp, Some(pin), cube_id)
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
    network: Network,
    fingerprint: Fingerprint,
    pin: &str,
    cube_id: &str,
) -> ConnectEncryptionKey {
    use crate::services::connect::crypto::CubeEncryptionKey;
    use coincube_core::signer::{MasterSigner, SignerError};

    let loaded = match crate::app::session::unlocked_signer(cube_id, fingerprint) {
        Some(signer) => Ok(signer),
        None => MasterSigner::from_datadir_by_fingerprint(
            datadir_root,
            network,
            fingerprint,
            Some(pin),
            cube_id,
        ),
    };

    match loaded {
        Ok(signer) => ConnectEncryptionKey::Derived(
            CubeEncryptionKey::derive(&signer, network).public_key_hex(),
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
    pub id: String,
    pub name: String,
    pub network: Network,
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
    /// All wallets (Vault, Liquid, Spark) derive from this single seed.
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
    /// Whether the one-time "turn on recovery alerts?" consent prompt has been
    /// answered for this Cube's Vault (PLAN-recovery-alerts-cleanup PR 3). Once
    /// `true` — whether the user accepted or declined — the prompt never fires
    /// again: a decline is durable, not nagging. `false`/absent = not yet asked.
    /// The residual nudge (a banner on the Recovery settings card when keyholders
    /// exist and alerts are off) is derived from live state, independent of this.
    #[serde(default)]
    pub recovery_alerts_prompt_answered: bool,
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PendingLiquidToVaultTransfer {
    pub swap_id: String,
    pub amount_sat: u64,
}

impl CubeSettings {
    /// Create a new `CubeSettings` with a caller-supplied id string, taken
    /// verbatim — no round-trip through `uuid::Uuid`, so the byte-exact value
    /// is preserved. Used by Cube recovery, where the restored Cube's id must
    /// string-match the server-side record for the idempotent `register_cube`
    /// call to reactivate the original Cube instead of minting a duplicate
    /// (parsing + re-formatting would normalize case; a malformed value would
    /// force a lossy fallback).
    pub fn new_with_raw_id(id: String, name: String, network: Network) -> Self {
        Self {
            id,
            name,
            network,
            created_at: chrono::Utc::now().timestamp(),
            vault_wallet_id: None,
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
            passkey_metadata: None,
            allow_random_grid_phrase: false,
            balance_masked: false,
            recovery_kit_last_backed_up_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_keychain_descriptor_fingerprint: None,
            recovery_alerts_prompt_answered: false,
            creation_backup_bypass: None,
            // Set explicitly by the creation flow; `new_*` is also used to
            // reconstruct Cubes in restore paths, which must not be gated.
            creation_backup_required: false,
            duress_slot_file: None,
        }
    }

    /// Create a new `CubeSettings` with a caller-supplied UUID.
    ///
    /// The frontend should generate this UUID before initiating the creation
    /// request so that retries reuse the same identifier (idempotent creation).
    pub fn new_with_id(id: uuid::Uuid, name: String, network: Network) -> Self {
        Self::new_with_raw_id(id.to_string(), name, network)
    }

    pub fn new(name: String, network: Network) -> Self {
        Self::new_with_id(uuid::Uuid::new_v4(), name, network)
    }

    pub fn with_vault(mut self, wallet_id: WalletId) -> Self {
        self.vault_wallet_id = Some(wallet_id);
        self
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
    /// device**, by either method. We treat a recorded backed-up descriptor
    /// fingerprint as the CRK-presence signal — set on a successful kit upload
    /// (password slot) or phone seal (keychain slot) and cleared on delete. Both
    /// slots count: a keychain-only backup is a recovery kit too, so keying off
    /// the password slot alone would make the Home card read "no recovery kit"
    /// while Settings (which reads the server per-method status) reads "backed
    /// up" (keychain-crk-status-fixes). This stays a **local** heuristic: it
    /// reflects backups made from this device, not the server-authoritative
    /// state, so an other-device backup can still read `false` here — the same
    /// limitation the password slot has always had. Distinct from
    /// [`backed_up`](Self::backed_up), which tracks the *local* seed-phrase
    /// backup, not the server-side recovery kit.
    pub fn has_recovery_kit(&self) -> bool {
        self.recovery_kit_last_backed_up_descriptor_fingerprint
            .is_some()
            || self
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .is_some()
    }

    /// Classifies this Cube's relationship to Connect for the card indicator
    /// (Phase 1 of duress mode). Users must be able to tell at a glance whether
    /// a Cube has a recovery kit before they're told what a duress wipe costs.
    pub fn connect_state(&self) -> CubeConnectState {
        if !self.remote_synced {
            CubeConnectState::Sovereign
        } else if self.has_recovery_kit() {
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

    /// Convert this cube's network to the API network string.
    pub fn api_network_string(&self) -> String {
        network_to_api_string(self.network)
    }
}

/// Convert a `Network` to the API network string used by the Connect backend.
pub fn network_to_api_string(network: Network) -> String {
    match network {
        Network::Bitcoin => "mainnet",
        Network::Testnet => "testnet",
        Network::Testnet4 => "testnet4",
        Network::Signet => "signet",
        Network::Regtest => "regtest",
    }
    .to_string()
}

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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletSettings {
    pub name: String,
    pub alias: Option<String>,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub remote_backend_auth: Option<AuthConfig>,
    /// Start internal bitcoind executable.
    /// if None, the app must refer to the gui.toml start_internal_bitcoind field.
    pub start_internal_bitcoind: Option<bool>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletId {
    pub timestamp: Option<i64>,
    pub descriptor_checksum: String,
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
        self.timestamp.is_none()
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Provider {
    pub uuid: String,
    pub name: String,
}

impl From<backend::api::Provider> for Provider {
    fn from(provider: backend::api::Provider) -> Self {
        Self {
            uuid: provider.uuid,
            name: provider.name,
        }
    }
}

impl From<services::keys::api::Provider> for Provider {
    fn from(provider: services::keys::api::Provider) -> Self {
        Self {
            uuid: provider.uuid,
            name: provider.name,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct ProviderKey {
    pub uuid: String,
    pub token: String,
    pub provider: Provider,
}

impl From<backend::api::ProviderKey> for ProviderKey {
    fn from(pk: backend::api::ProviderKey) -> Self {
        Self {
            uuid: pk.uuid.clone(),
            token: pk.token.clone(),
            provider: pk.provider.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeySetting {
    pub name: String,
    pub master_fingerprint: Fingerprint,
    pub provider_key: Option<ProviderKey>,
    /// Whether this key is a Border Wallet key (derived transiently from grid pattern).
    #[serde(default)]
    pub is_border_wallet: bool,
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
            serde_json::json!({ "is_border_wallet": true })
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
            })
        } else {
            let is_border_wallet = metadata
                .get("is_border_wallet")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Some(Self {
                name,
                master_fingerprint: fg,
                provider_key: None,
                is_border_wallet,
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
    use crate::dir::CoincubeDirectory;
    use async_hwi::bitbox::{ConfigError, NoiseConfig, NoiseConfigData};
    use fs4::fs_std::FileExt;
    use serde::{Deserialize, Serialize};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;

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

    #[derive(Debug, Deserialize, Serialize, Default)]
    pub struct GlobalSettings {
        pub bitbox: Option<BitboxSettings>,
        pub window_config: Option<WindowConfig>,
        #[serde(default)]
        pub developer_mode: bool,
        #[serde(default)]
        pub account_tier: AccountTier,
        #[serde(default)]
        pub theme_mode: coincube_ui::theme::palette::ThemeMode,
        #[serde(default = "default_true")]
        pub show_direction_badges: bool,
    }

    fn default_true() -> bool {
        true
    }

    impl GlobalSettings {
        pub fn path(global_datadir: &CoincubeDirectory) -> PathBuf {
            global_datadir.path().join(DEFAULT_FILE_NAME)
        }

        pub fn load_window_config(path: &PathBuf) -> Option<WindowConfig> {
            let mut ret = None;
            if let Err(e) = Self::update(path, |s| ret = s.window_config.clone(), false) {
                tracing::error!("Failed to load window config: {e}");
            }
            ret
        }

        pub fn update_window_config(
            path: &PathBuf,
            window_config: &WindowConfig,
        ) -> Result<(), super::SettingsError> {
            Self::update(
                path,
                |s| s.window_config = Some(window_config.clone()),
                true,
            )
        }

        pub fn load_bitbox_settings(
            path: &PathBuf,
        ) -> Result<Option<BitboxSettings>, super::SettingsError> {
            let mut ret = None;
            Self::update(path, |s| ret = s.bitbox.clone(), false)?;
            Ok(ret)
        }

        pub fn load_developer_mode(path: &PathBuf) -> bool {
            let mut ret = false;
            if let Err(e) = Self::update(path, |s| ret = s.developer_mode, false) {
                tracing::error!("Failed to load developer mode setting: {e}");
            }
            ret
        }

        pub fn load_account_tier(path: &PathBuf) -> AccountTier {
            let mut ret = AccountTier::default();
            if let Err(e) = Self::update(path, |s| ret = s.account_tier, false) {
                tracing::error!("Failed to load account tier setting: {e}");
            }
            ret
        }

        pub fn update_account_tier(
            path: &PathBuf,
            tier: AccountTier,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.account_tier = tier, true)
        }

        pub fn update_developer_mode(
            path: &PathBuf,
            developer_mode: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.developer_mode = developer_mode, true)
        }

        pub fn load_show_direction_badges(path: &PathBuf) -> bool {
            let mut ret = true;
            if let Err(e) = Self::update(path, |s| ret = s.show_direction_badges, false) {
                tracing::error!("Failed to load show_direction_badges setting: {e}");
            }
            ret
        }

        pub fn update_show_direction_badges(
            path: &PathBuf,
            show: bool,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.show_direction_badges = show, true)
        }

        pub fn load_theme_mode(path: &PathBuf) -> coincube_ui::theme::palette::ThemeMode {
            let mut ret = coincube_ui::theme::palette::ThemeMode::default();
            if let Err(e) = Self::update(path, |s| ret = s.theme_mode, false) {
                tracing::error!("Failed to load theme mode setting: {e}");
            }
            ret
        }

        pub fn update_theme_mode(
            path: &PathBuf,
            mode: coincube_ui::theme::palette::ThemeMode,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.theme_mode = mode, true)
        }

        pub fn update_bitbox_settings(
            path: &PathBuf,
            bitbox: &BitboxSettings,
        ) -> Result<(), super::SettingsError> {
            Self::update(path, |s| s.bitbox = Some(bitbox.clone()), true)
        }

        pub fn update<F>(
            path: &PathBuf,
            mut update: F,
            mut write: bool,
        ) -> Result<(), super::SettingsError>
        where
            F: FnMut(&mut GlobalSettings),
        {
            let exists = path.is_file();

            let (mut global_settings, file) = if exists {
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .map_err(|e| super::SettingsError::ReadingFile(format!("Opening file: {e}")))?;

                file.lock_exclusive()
                    .map_err(|e| super::SettingsError::ReadingFile(format!("Locking file: {e}")))?;

                let mut content = String::new();
                file.read_to_string(&mut content)
                    .map_err(|e| super::SettingsError::ReadingFile(format!("Reading file: {e}")))?;

                if !write {
                    File::unlock(&file).map_err(|e| {
                        super::SettingsError::ReadingFile(format!("Unlocking file: {e}"))
                    })?;
                }

                (
                    serde_json::from_str::<GlobalSettings>(&content)
                        .map_err(|e| super::SettingsError::ReadingFile(e.to_string()))?,
                    Some(file),
                )
            } else {
                (GlobalSettings::default(), None)
            };

            update(&mut global_settings);

            if !exists
                && global_settings.bitbox.is_none()
                && global_settings.window_config.is_none()
                && !global_settings.developer_mode
                && global_settings.account_tier == AccountTier::Free
                && global_settings.theme_mode == coincube_ui::theme::palette::ThemeMode::default()
            {
                write = false;
            }

            if write {
                let mut file = if let Some(file) = file {
                    file
                } else {
                    let file = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(path)
                        .map_err(|e| {
                            super::SettingsError::WritingFile(format!("Opening file: {e}"))
                        })?;

                    file.lock_exclusive().map_err(|e| {
                        super::SettingsError::WritingFile(format!("Locking file: {e}"))
                    })?;
                    file
                };
                let content = serde_json::to_vec_pretty(&global_settings).map_err(|e| {
                    super::SettingsError::WritingFile(format!(
                        "Failed to serialize GlobalSettings: {e}"
                    ))
                })?;

                file.seek(SeekFrom::Start(0)).map_err(|e| {
                    super::SettingsError::WritingFile(format!("Failed to seek file: {e}"))
                })?;

                file.write_all(&content).map_err(|e| {
                    super::SettingsError::WritingFile(format!("Failed to write file: {e}"))
                })?;
                file.set_len(content.len() as u64).map_err(|e| {
                    super::SettingsError::WritingFile(format!("Failed to truncate file: {e}"))
                })?;
                File::unlock(&file).map_err(|e| {
                    super::SettingsError::WritingFile(format!("Unlocking file: {e}"))
                })?;
            }

            Ok(())
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
}

#[cfg(test)]
mod test {
    use super::global::{GlobalSettings, WindowConfig};
    use std::env;

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
    fn test_parse_global_config() {
        let _ = serde_json::from_str::<GlobalSettings>(RAW_GLOBAL_SETTINGS).unwrap();
    }

    #[test]
    fn test_update_global_config() {
        let path = env::current_dir()
            .unwrap()
            .join("test_assets")
            .join("global_settings.json");
        assert!(path.exists());

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
        .unwrap()
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
