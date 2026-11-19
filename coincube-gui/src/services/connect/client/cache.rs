use crate::dir::NetworkDirectory;
use async_fd_lock::LockWrite;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::SeekFrom;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::auth::AccessTokenResponse;

pub const CONNECT_CACHE_FILENAME: &str = "connect.json";
const REREGISTER_LOCK_FILENAME_PREFIX: &str = "connect-reregister-";

fn reregister_lock_path(network_dir: &NetworkDirectory, email: &str) -> std::path::PathBuf {
    let normalized = email.trim().to_ascii_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let suffix: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    network_dir
        .path()
        .join(format!("{REREGISTER_LOCK_FILENAME_PREFIX}{suffix}.lock"))
}

fn open_reregister_lock(
    network_dir: &NetworkDirectory,
    email: &str,
) -> std::io::Result<std::fs::File> {
    let path = reregister_lock_path(network_dir, email);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

pub(crate) fn has_live_reregister_claim_for_account(
    network_dir: &NetworkDirectory,
    account: &Account,
) -> bool {
    if account.reregister_claim.is_none() {
        return false;
    }
    // The About task holds this account's lock until its RPC and cache commit
    // finish. Another account's live request cannot revive a crashed claim.
    // A crash releases the OS lock even if the PID is reused. On any I/O
    // uncertainty, preserve the claim rather than replacing a live request.
    match open_reregister_lock(network_dir, &account.email).and_then(|file| {
        let free = file.try_lock_exclusive()?;
        if free {
            file.unlock()?;
        }
        Ok(!free)
    }) {
        Ok(live) => live,
        Err(e) => {
            // Claims are UUIDs without timestamps; log the lock path to make
            // a persistent fail-safe block diagnosable.
            tracing::warn!(
                "Cannot check Connect re-registration lock at {:?}: {e}",
                reregister_lock_path(network_dir, &account.email)
            );
            true
        }
    }
}

pub struct DeviceReregistrationClaim {
    id: String,
    lock: std::fs::File,
}

impl DeviceReregistrationClaim {
    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl std::ops::Deref for DeviceReregistrationClaim {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

impl Drop for DeviceReregistrationClaim {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

/// Opens `connect.json` for read/write, creating it owner-only. An existing
/// file is tightened to `0600` as well: the cache holds refresh tokens.
async fn open_owner_cache(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        // Create owner-only before the tokio open. Tokio's OpenOptions on
        // this toolchain cannot set the mode itself.
        let owned = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(owned)
                .map(drop)
        })
        .await
        .map_err(std::io::Error::other)??;
    }
    let file = opts.open(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .await?;
    }
    Ok(file)
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConnectCache {
    #[serde(default)]
    pub active_email: Option<String>,
    pub accounts: Vec<Account>,
}

impl ConnectCache {
    /// Returns the explicitly active account. Legacy single-account caches are
    /// unambiguous and remain eligible for automatic restoration.
    pub fn active_account(&self) -> Option<&Account> {
        match self.active_email.as_deref() {
            Some(email) => self.accounts.iter().find(|account| account.email == email),
            None if self.accounts.len() == 1 => self.accounts.first(),
            None => None,
        }
    }

    fn active_email_matches(&self, email: &str) -> bool {
        self.active_account()
            .is_some_and(|account| account.email == email)
    }

    fn upsert_credential(
        &mut self,
        email: &str,
        tokens: AccessTokenResponse,
        action: CacheAccountAction,
    ) {
        if let Some(c) = self.accounts.iter_mut().find(|c| c.email == email) {
            c.tokens = tokens;
        } else {
            self.accounts.push(Account {
                email: email.to_string(),
                tokens,
                device_id: None,
                last_seen_event_seq: None,
                reregister_claim: None,
            })
        }
        if action != CacheAccountAction::PreserveSelection {
            // Only new authentication supersedes a registration started by
            // the prior session. Restoring cached credentials still selects
            // the account, but must leave a live About claim intact.
            if let Some(account) = self.accounts.iter_mut().find(|a| a.email == email) {
                if action == CacheAccountAction::NewLogin {
                    if account.reregister_claim.is_some() {
                        account.device_id = None;
                    }
                    account.reregister_claim = None;
                }
            }
            self.active_email = Some(email.to_string());
        }
    }

    pub fn from_file(network_dir: &NetworkDirectory) -> Result<Self, ConnectCacheError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(CONNECT_CACHE_FILENAME);

        std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ConnectCacheError::NotFound,
                _ => ConnectCacheError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<ConnectCache>(&file_content).map_err(|e| {
                    ConnectCacheError::ReadingFile(format!("Parsing settings file: {}", e))
                })
            })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CacheAccountAction {
    PreserveSelection,
    Activate,
    NewLogin,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,
    pub tokens: AccessTokenResponse,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub last_seen_event_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reregister_claim: Option<String>,
}

impl Account {
    /// A fresh account record for the opt-in Testnet4 provisioning runner.
    #[cfg(feature = "integration-tests")]
    pub fn new_for_integration(email: String, tokens: AccessTokenResponse) -> Self {
        Self {
            email,
            tokens,
            device_id: None,
            last_seen_event_seq: None,
            reregister_claim: None,
        }
    }

    pub fn from_cache(
        network_dir: &NetworkDirectory,
        email: &str,
    ) -> Result<Option<Self>, ConnectCacheError> {
        ConnectCache::from_file(network_dir)
            .map(|cache| cache.accounts.into_iter().find(|c| c.email == email))
    }
}

pub async fn update_connect_cache(
    network_dir: &NetworkDirectory,
    current_tokens: &AccessTokenResponse,
    email: &str,
    action: CacheAccountAction,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    update_connect_cache_inner(network_dir, current_tokens, email, action, None).await
}

enum CacheUpdateGuard<'a> {
    LoginSnapshot(&'a ConnectCache),
    RestoreSnapshot(&'a ConnectCache),
    RefreshToken(&'a str),
}

/// Persist an in-app refresh only if the cache still has the bearer that was
/// refreshed. This preserves account selection, device ID and any About claim.
pub async fn persist_refreshed_tokens(
    network_dir: &NetworkDirectory,
    email: &str,
    previous_refresh_token: &str,
    tokens: &AccessTokenResponse,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    update_connect_cache_inner(
        network_dir,
        tokens,
        email,
        CacheAccountAction::PreserveSelection,
        Some(CacheUpdateGuard::RefreshToken(previous_refresh_token)),
    )
    .await
}

/// Adopt a keyring session on this network only while the cache selection and
/// bearer seen before the restore RPC are unchanged. A Home refresh can rotate
/// the global keyring independently of this network's connect.json.
pub async fn persist_restored_tokens(
    network_dir: &NetworkDirectory,
    email: &str,
    tokens: &AccessTokenResponse,
    started_from: &ConnectCache,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    update_connect_cache_inner(
        network_dir,
        tokens,
        email,
        CacheAccountAction::Activate,
        Some(CacheUpdateGuard::RestoreSnapshot(started_from)),
    )
    .await
}

/// A foreground OTP login may complete after another login. Its snapshot is
/// taken before OTP verification and checked under the cache write lock.
pub async fn update_connect_cache_for_login(
    network_dir: &NetworkDirectory,
    tokens: &AccessTokenResponse,
    email: &str,
    started_from: &ConnectCache,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    update_connect_cache_inner(
        network_dir,
        tokens,
        email,
        CacheAccountAction::NewLogin,
        Some(CacheUpdateGuard::LoginSnapshot(started_from)),
    )
    .await
}

async fn update_connect_cache_inner(
    network_dir: &NetworkDirectory,
    current_tokens: &AccessTokenResponse,
    email: &str,
    action: CacheAccountAction,
    guard: Option<CacheUpdateGuard<'_>>,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let mut file = open_owner_cache(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {}", e)))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {:?}", e)))?;

    let mut file_content = Vec::new();
    file.read_to_end(&mut file_content)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {}", e)))?;

    let mut cache = if file_content.is_empty() {
        ConnectCache::default()
    } else {
        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(cache) => cache,
            Err(e) => {
                if guard.is_some() {
                    return Err(ConnectCacheError::ReadingFile(format!(
                        "Parsing guarded Connect cache: {e}"
                    )));
                }
                tracing::warn!("Something wrong with Connect cache file: {:?}", e);
                tracing::warn!("Connect cache file is reset");
                ConnectCache::default()
            }
        }
    };

    let guarded = guard.is_some();
    validate_cache_update_guard(&cache, email, guard)?;

    let newer_cached_tokens = cache
        .accounts
        .iter()
        .find(|cred| cred.email == *email)
        .filter(|account| !guarded && current_tokens.expires_at < account.tokens.expires_at)
        .map(|account| account.tokens.clone());

    let tokens = if let Some(tokens) = newer_cached_tokens {
        tracing::debug!("Connect authentication tokens are up to date");
        tokens
    } else {
        current_tokens.clone()
    };

    cache.upsert_credential(email, tokens.clone(), action);

    let content = serde_json::to_vec_pretty(&cache).map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to serialize settings: {}", e))
    })?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        ConnectCacheError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {}", e)))?;

    Ok(tokens)
}

fn validate_cache_update_guard(
    cache: &ConnectCache,
    email: &str,
    guard: Option<CacheUpdateGuard<'_>>,
) -> Result<(), ConnectCacheError> {
    match guard {
        Some(CacheUpdateGuard::LoginSnapshot(started_from)) => {
            let before = started_from.accounts.iter().find(|a| a.email == *email);
            let now = cache.accounts.iter().find(|a| a.email == *email);
            if cache.active_email != started_from.active_email
                || now.map(|a| {
                    (
                        &a.tokens.access_token,
                        &a.tokens.refresh_token,
                        a.tokens.expires_at,
                    )
                }) != before.map(|a| {
                    (
                        &a.tokens.access_token,
                        &a.tokens.refresh_token,
                        a.tokens.expires_at,
                    )
                })
            {
                return Err(ConnectCacheError::Unexpected(
                    "Connect login was superseded while OTP verification was pending".into(),
                ));
            }
        }
        Some(CacheUpdateGuard::RestoreSnapshot(started_from)) => {
            fn account_tokens<'a>(
                state: &'a ConnectCache,
                email: &str,
            ) -> Option<(&'a str, &'a str, i64)> {
                state.accounts.iter().find(|a| a.email == email).map(|a| {
                    (
                        a.tokens.access_token.as_str(),
                        a.tokens.refresh_token.as_str(),
                        a.tokens.expires_at,
                    )
                })
            }
            let active_before = started_from.active_account().map(|a| a.email.as_str());
            let active_now = cache.active_account().map(|a| a.email.as_str());
            if cache.active_email != started_from.active_email
                || active_now != active_before
                || account_tokens(cache, email) != account_tokens(started_from, email)
                || active_before.is_some_and(|active| {
                    account_tokens(cache, active) != account_tokens(started_from, active)
                })
            {
                return Err(ConnectCacheError::Unexpected(
                    "Connect restore was superseded while refresh was pending".into(),
                ));
            }
        }
        Some(CacheUpdateGuard::RefreshToken(previous)) => {
            let current = cache.accounts.iter().find(|a| a.email == *email);
            if !cache.active_email_matches(email)
                || current.map(|a| a.tokens.refresh_token.as_str()) != Some(previous)
            {
                return Err(ConnectCacheError::Unexpected(
                    "Connect refresh was superseded by another session".into(),
                ));
            }
        }
        None => {}
    }
    Ok(())
}

/// Persist (or clear) the gRPC-issued `device_id` for the account matching
/// `email`. Pass `Some(id)` to set, `None` to clear so the next
/// `ensure_device_registered` call takes the register path instead of
/// short-circuiting on a stale entry.
///
/// Acquires a write lock on the cache file so the update is atomic against
/// concurrent `update_connect_cache` calls. Returns false when the account
/// is absent or an About re-registration owns the cache claim.
pub async fn set_device_id_for_email(
    network_dir: &NetworkDirectory,
    email: &str,
    device_id: Option<&str>,
) -> Result<bool, ConnectCacheError> {
    mutate_device_registration(
        network_dir,
        email,
        DeviceRegistrationUpdate::Set {
            device_id,
            claim: None,
            expected_refresh_token: None,
        },
    )
    .await
}

/// Commit a plain registration only for the same active bearer that started
/// its RPC. A later login cannot inherit an earlier RPC's device ID.
pub async fn set_device_id_for_email_if_token(
    network_dir: &NetworkDirectory,
    email: &str,
    device_id: &str,
    expected_refresh_token: &str,
) -> Result<bool, ConnectCacheError> {
    mutate_device_registration(
        network_dir,
        email,
        DeviceRegistrationUpdate::Set {
            device_id: Some(device_id),
            claim: None,
            expected_refresh_token: Some(expected_refresh_token),
        },
    )
    .await
}

/// Atomically claim a replacement while preserving the old id for crash
/// recovery. A subsequent foreground login invalidates the claim and old id,
/// even for the same email, so that login registers its own device.
pub async fn begin_device_reregistration(
    network_dir: &NetworkDirectory,
    email: &str,
    expected_id: &str,
) -> Result<Option<DeviceReregistrationClaim>, ConnectCacheError> {
    let lock = open_reregister_lock(network_dir, email)
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening registration lock: {e}")))?;
    if !lock
        .try_lock_exclusive()
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking registration claim: {e}")))?
    {
        return Ok(None);
    }
    let claim = uuid::Uuid::new_v4().to_string();
    let applied = mutate_device_registration(
        network_dir,
        email,
        DeviceRegistrationUpdate::Begin {
            expected_id,
            claim: &claim,
        },
    )
    .await?;
    Ok(applied.then_some(DeviceReregistrationClaim { id: claim, lock }))
}

/// Commit or roll back only while the exact re-registration claim still owns
/// the cache. A stale RPC cannot write into a newer login's account record.
pub async fn finish_device_reregistration(
    network_dir: &NetworkDirectory,
    email: &str,
    claim: &str,
    device_id: Option<&str>,
) -> Result<bool, ConnectCacheError> {
    mutate_device_registration(
        network_dir,
        email,
        DeviceRegistrationUpdate::Set {
            device_id,
            claim: Some(claim),
            expected_refresh_token: None,
        },
    )
    .await
}

enum DeviceRegistrationUpdate<'a> {
    Begin {
        expected_id: &'a str,
        claim: &'a str,
    },
    Set {
        device_id: Option<&'a str>,
        claim: Option<&'a str>,
        expected_refresh_token: Option<&'a str>,
    },
}

async fn mutate_device_registration(
    network_dir: &NetworkDirectory,
    email: &str,
    update: DeviceRegistrationUpdate<'_>,
) -> Result<bool, ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let mut file = open_owner_cache(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {}", e)))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {:?}", e)))?;

    let mut file_content = Vec::new();
    file.read_to_end(&mut file_content)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {}", e)))?;
    let mut cache = if file_content.is_empty() {
        // Freshly `create(true)`d (or never written) — start empty.
        ConnectCache::default()
    } else {
        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Connect cache at {:?} is corrupt ({}); continuing with an \
                     empty cache for this update",
                    path,
                    e,
                );
                ConnectCache::default()
            }
        }
    };

    let accepted = match apply_device_registration_update(network_dir, &mut cache, email, update) {
        DeviceRegistrationMutation::Unchanged(accepted) => return Ok(accepted),
        DeviceRegistrationMutation::Changed(accepted) => accepted,
    };

    let content = serde_json::to_vec_pretty(&cache)
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to serialize cache: {}", e)))?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;
    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        ConnectCacheError::WritingFile(e.to_string())
    })?;
    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {}", e)))?;
    Ok(accepted)
}

enum DeviceRegistrationMutation {
    Unchanged(bool),
    Changed(bool),
}

fn apply_device_registration_update(
    network_dir: &NetworkDirectory,
    cache: &mut ConnectCache,
    email: &str,
    update: DeviceRegistrationUpdate<'_>,
) -> DeviceRegistrationMutation {
    let is_active = cache.active_email_matches(email);
    let Some(account) = cache.accounts.iter_mut().find(|a| a.email == email) else {
        return DeviceRegistrationMutation::Unchanged(false);
    };
    let mut accepted = true;
    match update {
        DeviceRegistrationUpdate::Begin { expected_id, claim } => {
            if !is_active || account.device_id.as_deref() != Some(expected_id) {
                return DeviceRegistrationMutation::Unchanged(false);
            }
            account.reregister_claim = Some(claim.to_string());
            DeviceRegistrationMutation::Changed(accepted)
        }
        DeviceRegistrationUpdate::Set {
            device_id,
            claim,
            expected_refresh_token,
        } => {
            if let Some(claim) = claim {
                if account.reregister_claim.as_deref() != Some(claim) {
                    return DeviceRegistrationMutation::Unchanged(false);
                }
                // The account lost selection while its RPC was in flight.
                // Retire only this request's claim; keep its previous device
                // id and never install the late RPC result.
                if is_active {
                    account.device_id = device_id.map(str::to_string);
                } else {
                    accepted = false;
                }
                account.reregister_claim = None;
                DeviceRegistrationMutation::Changed(accepted)
            } else {
                apply_unclaimed_device_registration(
                    network_dir,
                    account,
                    is_active,
                    device_id,
                    expected_refresh_token,
                )
            }
        }
    }
}

fn apply_unclaimed_device_registration(
    network_dir: &NetworkDirectory,
    account: &mut Account,
    is_active: bool,
    device_id: Option<&str>,
    expected_refresh_token: Option<&str>,
) -> DeviceRegistrationMutation {
    if expected_refresh_token
        .is_some_and(|expected| !is_active || account.tokens.refresh_token != expected)
    {
        return DeviceRegistrationMutation::Unchanged(false);
    }
    if has_live_reregister_claim_for_account(network_dir, account) {
        return DeviceRegistrationMutation::Unchanged(false);
    }
    if account.device_id.as_deref() == device_id && account.reregister_claim.is_none() {
        return DeviceRegistrationMutation::Unchanged(true);
    }
    account.device_id = device_id.map(str::to_string);
    // An exited process's claim is abandoned. Its late completion
    // still needs the exact claim to commit.
    account.reregister_claim = None;
    DeviceRegistrationMutation::Changed(true)
}

/// Persist the highest event_seq observed on the realtime stream for the
/// account matching `email`. Idempotent — only writes when the new value is
/// strictly greater, so concurrent stream writers can't move the cursor
/// backwards.
pub async fn set_last_seen_event_seq_for_email(
    network_dir: &NetworkDirectory,
    email: &str,
    event_seq: i64,
) -> Result<(), ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let mut file = open_owner_cache(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {}", e)))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {:?}", e)))?;

    let mut file_content = Vec::new();
    file.read_to_end(&mut file_content)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {}", e)))?;
    let mut cache = if file_content.is_empty() {
        // Freshly `create(true)`d (or never written) — start empty.
        ConnectCache::default()
    } else {
        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Connect cache at {:?} is corrupt ({}); continuing with an \
                     empty cache for this update",
                    path,
                    e,
                );
                ConnectCache::default()
            }
        }
    };

    let Some(account) = cache.accounts.iter_mut().find(|a| a.email == email) else {
        return Ok(());
    };
    if account.last_seen_event_seq.unwrap_or(0) >= event_seq {
        return Ok(());
    }
    account.last_seen_event_seq = Some(event_seq);

    let content = serde_json::to_vec_pretty(&cache)
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to serialize cache: {}", e)))?;
    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;
    file.write_all(&content)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(e.to_string()))?;
    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {}", e)))?;
    Ok(())
}

pub async fn filter_connect_cache(
    network_dir: &NetworkDirectory,
    emails: &HashSet<String>,
) -> Result<(), ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = open_owner_cache(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {}", e)))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {:?}", e)))?;

    let mut cache = if file_exists {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {}", e)))?;

        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(cache) => cache,
            Err(e) => {
                tracing::warn!("Something wrong with Connect cache file: {:?}", e);
                tracing::warn!("Connect cache file is reset");
                ConnectCache::default()
            }
        }
    } else {
        ConnectCache::default()
    };

    cache.accounts.retain(|a| emails.contains(&a.email));
    if cache
        .active_email
        .as_ref()
        .is_some_and(|email| !emails.contains(email))
    {
        cache.active_email = None;
    }

    let content = serde_json::to_vec_pretty(&cache).map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to serialize settings: {}", e))
    })?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        ConnectCacheError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {}", e)))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub enum ConnectCacheError {
    NotFound,
    ReadingFile(String),
    WritingFile(String),
    Unexpected(String),
}
impl std::fmt::Display for ConnectCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "ConnectCache file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        begin_device_reregistration, finish_device_reregistration, persist_refreshed_tokens,
        persist_restored_tokens, set_device_id_for_email, set_device_id_for_email_if_token,
        update_connect_cache, update_connect_cache_for_login, AccessTokenResponse,
        CacheAccountAction, ConnectCache, CONNECT_CACHE_FILENAME,
    };
    use crate::dir::NetworkDirectory;

    fn tokens(label: &str) -> AccessTokenResponse {
        AccessTokenResponse {
            access_token: format!("{label}-access"),
            refresh_token: format!("{label}-refresh"),
            expires_at: 123,
        }
    }

    fn test_network_dir() -> NetworkDirectory {
        let path = std::env::temp_dir().join(format!(
            "connect-registration-cache-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        NetworkDirectory::new(path)
    }

    #[tokio::test]
    async fn cached_restore_preserves_live_claim_and_previous_device_id() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        update_connect_cache(
            &dir,
            &tokens("restored"),
            email,
            CacheAccountAction::Activate,
        )
        .await
        .unwrap();

        let restored = ConnectCache::from_file(&dir).unwrap();
        let account = restored.active_account().unwrap();
        assert_eq!(restored.active_email.as_deref(), Some(email));
        assert_eq!(account.device_id.as_deref(), Some("device-a"));
        assert_eq!(account.reregister_claim.as_deref(), Some(claim.as_str()));
        assert!(
            finish_device_reregistration(&dir, email, &claim, Some("device-b"))
                .await
                .unwrap()
        );
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("device-b")
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn slower_login_cannot_supersede_newer_tokens_and_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("original"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let stale_login_snapshot = ConnectCache::from_file(&dir).unwrap();

        let mut newer = tokens("newer");
        newer.expires_at = 300;
        update_connect_cache_for_login(&dir, &newer, email, &stale_login_snapshot)
            .await
            .unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        assert!(update_connect_cache_for_login(
            &dir,
            &tokens("slower"),
            email,
            &stale_login_snapshot,
        )
        .await
        .is_err());
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(restored.active_email.as_deref(), Some(email));
        assert_eq!(restored.accounts[0].tokens.access_token, newer.access_token);
        assert_eq!(restored.accounts[0].device_id.as_deref(), Some("device-a"));
        assert_eq!(
            restored.accounts[0].reregister_claim.as_deref(),
            Some(claim.as_str())
        );
        let mut switched = restored;
        switched.upsert_credential("bob@example.com", tokens("b"), CacheAccountAction::NewLogin);
        std::fs::write(&path, serde_json::to_vec(&switched).unwrap()).unwrap();
        assert!(update_connect_cache_for_login(
            &dir,
            &tokens("slower"),
            email,
            &stale_login_snapshot,
        )
        .await
        .is_err());
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_email
                .as_deref(),
            Some("bob@example.com")
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn deliberate_login_supersedes_claim_even_with_shorter_token_lifetime() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        let mut old = tokens("old");
        old.expires_at = 300;
        cache.upsert_credential(email, old, CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let snapshot = ConnectCache::from_file(&dir).unwrap();
        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        let returned = update_connect_cache_for_login(&dir, &tokens("new"), email, &snapshot)
            .await
            .unwrap();
        assert_eq!(returned.access_token, "new-access");
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert!(restored.accounts[0].reregister_claim.is_none());
        assert!(restored.accounts[0].device_id.is_none());
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn restore_adopts_rotated_keyring_token_preserving_claim_and_rejects_replacement() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("cached"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        let snapshot = ConnectCache::from_file(&dir).unwrap();
        persist_restored_tokens(&dir, email, &tokens("keyring-refreshed"), &snapshot)
            .await
            .unwrap();
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            restored.active_account().unwrap().tokens.refresh_token,
            "keyring-refreshed-refresh"
        );
        assert_eq!(
            restored.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            restored
                .active_account()
                .unwrap()
                .reregister_claim
                .as_deref(),
            Some(claim.as_str())
        );
        assert!(
            persist_restored_tokens(&dir, email, &tokens("late"), &snapshot)
                .await
                .is_err()
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn restore_admits_empty_network_cache_but_rejects_concurrent_new_login() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let empty = ConnectCache::default();
        persist_restored_tokens(&dir, "alice@example.com", &tokens("alice"), &empty)
            .await
            .unwrap();
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_email
                .as_deref(),
            Some("alice@example.com")
        );
        std::fs::remove_file(&path).unwrap();
        let mut concurrent = ConnectCache::default();
        concurrent.upsert_credential(
            "bob@example.com",
            tokens("bob"),
            CacheAccountAction::NewLogin,
        );
        std::fs::write(&path, serde_json::to_vec(&concurrent).unwrap()).unwrap();
        assert!(
            persist_restored_tokens(&dir, "alice@example.com", &tokens("late-alice"), &empty)
                .await
                .is_err()
        );
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_email
                .as_deref(),
            Some("bob@example.com")
        );
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn restore_selects_keyring_account_without_erasing_other_account_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let mut cache = ConnectCache::default();
        cache.upsert_credential(
            "bob@example.com",
            tokens("bob"),
            CacheAccountAction::NewLogin,
        );
        cache.accounts[0].device_id = Some("bob-device".into());
        cache.accounts[0].reregister_claim = Some("bob-claim".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let snapshot = ConnectCache::from_file(&dir).unwrap();
        persist_restored_tokens(&dir, "alice@example.com", &tokens("alice"), &snapshot)
            .await
            .unwrap();
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(restored.active_email.as_deref(), Some("alice@example.com"));
        let bob = restored
            .accounts
            .iter()
            .find(|a| a.email == "bob@example.com")
            .unwrap();
        assert_eq!(bob.device_id.as_deref(), Some("bob-device"));
        assert_eq!(bob.reregister_claim.as_deref(), Some("bob-claim"));
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn refresh_updates_tokens_without_disturbing_live_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("old"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();

        persist_refreshed_tokens(&dir, email, "old-refresh", &tokens("refreshed"))
            .await
            .unwrap();
        let updated = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(updated.accounts[0].tokens.access_token, "refreshed-access");
        assert_eq!(updated.accounts[0].device_id.as_deref(), Some("device-a"));
        assert_eq!(
            updated.accounts[0].reregister_claim.as_deref(),
            Some(claim.as_str())
        );
        assert!(
            persist_refreshed_tokens(&dir, email, "old-refresh", &tokens("late"))
                .await
                .is_err()
        );
        let unchanged = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            unchanged.accounts[0].tokens.access_token,
            "refreshed-access"
        );
        assert_eq!(
            unchanged.accounts[0].reregister_claim.as_deref(),
            Some(claim.as_str())
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn late_plain_registration_cannot_write_into_new_login() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("old"), CacheAccountAction::NewLogin);
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let snapshot = ConnectCache::from_file(&dir).unwrap();
        update_connect_cache_for_login(&dir, &tokens("new"), email, &snapshot)
            .await
            .unwrap();

        assert!(
            !set_device_id_for_email_if_token(&dir, email, "late-device", "old-refresh")
                .await
                .unwrap()
        );
        let current = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(current.accounts[0].tokens.access_token, "new-access");
        assert!(current.accounts[0].device_id.is_none());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn inactive_exact_claim_is_retired_without_committing_late_device() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();

        let mut switched = ConnectCache::from_file(&dir).unwrap();
        switched.upsert_credential("bob@example.com", tokens("b"), CacheAccountAction::NewLogin);
        switched.accounts[1].device_id = Some("device-b".into());
        std::fs::write(&path, serde_json::to_vec(&switched).unwrap()).unwrap();
        assert!(
            !finish_device_reregistration(&dir, email, &claim, Some("late-device"))
                .await
                .unwrap()
        );
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(restored.active_email.as_deref(), Some("bob@example.com"));
        assert_eq!(restored.accounts[0].device_id.as_deref(), Some("device-a"));
        assert!(restored.accounts[0].reregister_claim.is_none());
        assert!(
            !finish_device_reregistration(&dir, email, &claim, Some("later-device"))
                .await
                .unwrap()
        );
        drop(claim);
        let b_claim = begin_device_reregistration(&dir, "bob@example.com", "device-b")
            .await
            .unwrap()
            .unwrap();
        let while_b_owns_lock = ConnectCache::from_file(&dir).unwrap();
        assert!(!super::has_live_reregister_claim_for_account(
            &dir,
            &while_b_owns_lock.accounts[0]
        ));
        assert!(super::has_live_reregister_claim_for_account(
            &dir,
            &while_b_owns_lock.accounts[1]
        ));
        drop(b_claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn another_accounts_lock_does_not_revive_abandoned_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let mut cache = ConnectCache::default();
        cache.upsert_credential(
            "alice@example.com",
            tokens("a"),
            CacheAccountAction::NewLogin,
        );
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let a_claim = begin_device_reregistration(&dir, "alice@example.com", "device-a")
            .await
            .unwrap()
            .unwrap();
        drop(a_claim); // Alice's process exited without finishing.

        let mut switched = ConnectCache::from_file(&dir).unwrap();
        switched.upsert_credential("bob@example.com", tokens("b"), CacheAccountAction::NewLogin);
        switched.accounts[1].device_id = Some("device-b".into());
        std::fs::write(&path, serde_json::to_vec(&switched).unwrap()).unwrap();
        let b_claim = begin_device_reregistration(&dir, "bob@example.com", "device-b")
            .await
            .unwrap()
            .unwrap();
        let current = ConnectCache::from_file(&dir).unwrap();
        assert!(current.accounts[0].reregister_claim.is_some());
        assert!(!super::has_live_reregister_claim_for_account(
            &dir,
            &current.accounts[0]
        ));
        assert!(super::has_live_reregister_claim_for_account(
            &dir,
            &current.accounts[1]
        ));
        drop(b_claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn cached_restore_preserves_previous_id_after_abandoned_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        cache.accounts[0].reregister_claim = Some("abandoned-claim".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        update_connect_cache(
            &dir,
            &tokens("restored"),
            email,
            CacheAccountAction::Activate,
        )
        .await
        .unwrap();
        let restored = ConnectCache::from_file(&dir).unwrap();
        let account = restored.active_account().unwrap();
        assert_eq!(account.device_id.as_deref(), Some("device-a"));
        assert_eq!(account.reregister_claim.as_deref(), Some("abandoned-claim"));
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn stale_reregistration_cannot_overwrite_same_email_login_or_rollback() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("device-a")
        );

        // B signs in to the same email before A's RegisterDevice RPC returns.
        let mut b_cache = ConnectCache::from_file(&dir).unwrap();
        b_cache.upsert_credential(email, tokens("b"), CacheAccountAction::NewLogin);
        assert!(b_cache.active_account().unwrap().device_id.is_none());
        std::fs::write(&path, serde_json::to_vec(&b_cache).unwrap()).unwrap();
        assert!(
            !finish_device_reregistration(&dir, email, &claim, Some("late-a"))
                .await
                .unwrap()
        );
        set_device_id_for_email(&dir, email, Some("device-b"))
            .await
            .unwrap();
        assert!(
            !finish_device_reregistration(&dir, email, &claim, Some("device-a"))
                .await
                .unwrap()
        );

        // A fresh launch reads B's id from the actual cache file.
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            restored.active_account().unwrap().device_id.as_deref(),
            Some("device-b")
        );
        assert_eq!(
            restored.active_account().unwrap().tokens.access_token,
            "b-access"
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn failed_clear_does_not_claim_or_report_registration_ready() {
        let dir = test_network_dir();
        // A directory at the cache path deterministically prevents opening it
        // as a writable file, independent of the test user's permissions.
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        std::fs::create_dir(&path).unwrap();
        assert!(
            begin_device_reregistration(&dir, "alice@example.com", "device-a")
                .await
                .is_err()
        );
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn failed_rpc_restores_old_id_only_for_own_claim() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        assert!(
            finish_device_reregistration(&dir, email, &claim, Some("device-a"))
                .await
                .unwrap()
        );
        assert!(
            !finish_device_reregistration(&dir, email, &claim, Some("late-a"))
                .await
                .unwrap()
        );
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("device-a")
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn background_bootstrap_cannot_supersede_live_about_claim() {
        use std::sync::Arc;

        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        let result = crate::services::connect::grpc::bootstrap::ensure_device_registered(
            "invalid://must-not-reach-network",
            Arc::new(tokio::sync::RwLock::new(tokens("a"))),
            &dir,
            email,
            "test desktop".into(),
            "test".into(),
            "test".into(),
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::services::connect::grpc::bootstrap::BootstrapError::Superseded)
        ));
        assert!(
            !set_device_id_for_email(&dir, email, Some("background-device"))
                .await
                .unwrap()
        );
        let claimed = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            claimed.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            claimed
                .active_account()
                .unwrap()
                .reregister_claim
                .as_deref(),
            Some(claim.as_str())
        );

        assert!(
            finish_device_reregistration(&dir, email, &claim, Some("device-a-new"))
                .await
                .unwrap()
        );
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("device-a-new")
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn reused_pid_does_not_keep_abandoned_claim_alive() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        // The numeric PID is alive (this test process), but it is not the
        // original owner. The released OS lock is authoritative.
        let prior_claim = format!("{}:prior-nonce:claim-a", std::process::id());
        cache.accounts[0].reregister_claim = Some(prior_claim.clone());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        assert!(!super::has_live_reregister_claim_for_account(
            &dir,
            &cache.accounts[0]
        ));
        assert_eq!(
            ConnectCache::from_file(&dir)
                .unwrap()
                .active_account()
                .unwrap()
                .device_id
                .as_deref(),
            Some("device-a")
        );
        assert!(
            set_device_id_for_email(&dir, email, Some("recovered-device"))
                .await
                .unwrap()
        );
        assert!(
            !finish_device_reregistration(&dir, email, &prior_claim, Some("late-a"))
                .await
                .unwrap()
        );
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            restored.active_account().unwrap().device_id.as_deref(),
            Some("recovered-device")
        );
        assert!(restored
            .active_account()
            .unwrap()
            .reregister_claim
            .is_none());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn hold_reregister_lock_child() {
        use std::io::{Read, Write};
        let Ok(path) = std::env::var("COINCUBE_TEST_REREGISTER_LOCK_DIR") else {
            return;
        };
        let dir = NetworkDirectory::new(path.into());
        let lock = super::open_reregister_lock(&dir, "alice@example.com").unwrap();
        fs4::fs_std::FileExt::lock_exclusive(&lock).unwrap();
        println!("REREGISTER_LOCK_READY");
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read(&mut [0u8; 1]);
        fs4::fs_std::FileExt::unlock(&lock).unwrap();
    }

    #[tokio::test]
    async fn second_live_process_cannot_take_over_about_claim() {
        use std::io::BufRead;
        use std::sync::Arc;

        let dir = test_network_dir();
        let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "services::connect::client::cache::tests::hold_reregister_lock_child",
                "--nocapture",
            ])
            .env("COINCUBE_TEST_REREGISTER_LOCK_DIR", dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = owner.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(output).lines();
        assert!(lines.any(|line| line.unwrap().contains("REREGISTER_LOCK_READY")));
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let claim = "other-process:claim-a".to_string();
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::NewLogin);
        cache.accounts[0].device_id = Some("device-a".into());
        cache.accounts[0].reregister_claim = Some(claim.clone());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        assert!(super::has_live_reregister_claim_for_account(
            &dir,
            &cache.accounts[0]
        ));
        assert!(begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .is_none());
        let result = crate::services::connect::grpc::bootstrap::ensure_device_registered(
            "invalid://must-not-reach-network",
            Arc::new(tokio::sync::RwLock::new(tokens("a"))),
            &dir,
            email,
            "test desktop".into(),
            "test".into(),
            "test".into(),
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::services::connect::grpc::bootstrap::BootstrapError::Superseded)
        ));
        assert!(!set_device_id_for_email(&dir, email, Some("device-b"))
            .await
            .unwrap());
        let persisted = ConnectCache::from_file(&dir).unwrap();
        assert_eq!(
            persisted.active_account().unwrap().device_id.as_deref(),
            Some("device-a")
        );
        assert_eq!(
            persisted
                .active_account()
                .unwrap()
                .reregister_claim
                .as_deref(),
            Some(claim.as_str())
        );

        owner.kill().unwrap();
        owner.wait().unwrap();
        let new_claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(new_claim.as_str(), claim.as_str());
        assert!(
            finish_device_reregistration(&dir, email, &new_claim, Some("device-b"))
                .await
                .unwrap()
        );
        drop(new_claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn legacy_single_account_can_begin_and_finish_reregistration() {
        let dir = test_network_dir();
        let path = dir.path().join(CONNECT_CACHE_FILENAME);
        let email = "alice@example.com";
        let mut cache = ConnectCache::default();
        cache.upsert_credential(email, tokens("a"), CacheAccountAction::PreserveSelection);
        cache.accounts[0].device_id = Some("device-a".into());
        assert!(cache.active_email.is_none());
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let claim = begin_device_reregistration(&dir, email, "device-a")
            .await
            .unwrap()
            .unwrap();
        assert!(
            finish_device_reregistration(&dir, email, &claim, Some("device-b"))
                .await
                .unwrap()
        );
        let restored = ConnectCache::from_file(&dir).unwrap();
        assert!(restored.active_email.is_none());
        assert_eq!(
            restored.active_account().unwrap().device_id.as_deref(),
            Some("device-b")
        );
        drop(claim);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    fn cache_from_json(json: &str) -> ConnectCache {
        serde_json::from_str(json).expect("valid Connect cache")
    }

    #[test]
    fn legacy_single_account_is_restored() {
        let cache = cache_from_json(
            r#"{
                "accounts": [{
                    "email": "alice@example.com",
                    "tokens": {
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "expires_at": 123
                    }
                }]
            }"#,
        );

        assert_eq!(
            cache.active_account().map(|account| account.email.as_str()),
            Some("alice@example.com")
        );
    }

    #[test]
    fn legacy_multi_account_cache_is_not_restored_arbitrarily() {
        let cache = cache_from_json(
            r#"{
                "accounts": [
                    {
                        "email": "alice@example.com",
                        "tokens": {
                            "access_token": "alice-access",
                            "refresh_token": "alice-refresh",
                            "expires_at": 123
                        }
                    },
                    {
                        "email": "bob@example.com",
                        "tokens": {
                            "access_token": "bob-access",
                            "refresh_token": "bob-refresh",
                            "expires_at": 456
                        }
                    }
                ]
            }"#,
        );

        assert!(cache.active_account().is_none());
    }

    #[test]
    fn explicit_active_email_selects_matching_account() {
        let cache = cache_from_json(
            r#"{
                "active_email": "bob@example.com",
                "accounts": [
                    {
                        "email": "alice@example.com",
                        "tokens": {
                            "access_token": "alice-access",
                            "refresh_token": "alice-refresh",
                            "expires_at": 123
                        }
                    },
                    {
                        "email": "bob@example.com",
                        "tokens": {
                            "access_token": "bob-access",
                            "refresh_token": "bob-refresh",
                            "expires_at": 456
                        }
                    }
                ]
            }"#,
        );

        assert_eq!(
            cache.active_account().map(|account| account.email.as_str()),
            Some("bob@example.com")
        );
    }

    #[test]
    fn credential_upsert_marks_account_active() {
        let mut cache = ConnectCache::default();
        cache.upsert_credential(
            "alice@example.com",
            AccessTokenResponse {
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                expires_at: 123,
            },
            CacheAccountAction::NewLogin,
        );

        assert_eq!(cache.active_email.as_deref(), Some("alice@example.com"));
        assert_eq!(
            cache.active_account().map(|account| account.email.as_str()),
            Some("alice@example.com")
        );
    }

    #[test]
    fn background_credential_upsert_preserves_active_account() {
        let mut cache = ConnectCache::default();
        cache.upsert_credential(
            "alice@example.com",
            AccessTokenResponse {
                access_token: "alice-access".to_string(),
                refresh_token: "alice-refresh".to_string(),
                expires_at: 123,
            },
            CacheAccountAction::NewLogin,
        );
        cache.upsert_credential(
            "bob@example.com",
            AccessTokenResponse {
                access_token: "bob-access".to_string(),
                refresh_token: "bob-refresh".to_string(),
                expires_at: 456,
            },
            CacheAccountAction::PreserveSelection,
        );

        assert_eq!(cache.active_email.as_deref(), Some("alice@example.com"));
        assert_eq!(cache.accounts.len(), 2);
        assert_eq!(
            cache.active_account().map(|account| account.email.as_str()),
            Some("alice@example.com")
        );
    }
}
