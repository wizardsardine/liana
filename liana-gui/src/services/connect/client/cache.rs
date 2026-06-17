use crate::dir::NetworkDirectory;
use async_fd_lock::LockWrite;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::SeekFrom;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::auth::{AccessTokenResponse, AuthClient, AuthError};

pub const CONNECT_CACHE_FILENAME: &str = "connect.json";

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConnectCache {
    pub accounts: Vec<Account>,
}

impl ConnectCache {
    /// Upsert tokens for the row matching `email`. When `user_id` is known
    /// (the caller has just spoken to the backend), stamp it onto the row at
    /// write time — both for fresh inserts and to promote legacy rows that
    /// still lack a user_id. Refresh callers that don't know the user_id pass
    /// `None`; in that case the existing user_id is preserved.
    fn upsert_credential(
        &mut self,
        user_id: Option<&str>,
        email: &str,
        tokens: AccessTokenResponse,
    ) {
        if let Some(c) = self.accounts.iter_mut().find(|c| c.email == email) {
            c.tokens = tokens;
            if let Some(uid) = user_id {
                c.user_id = Some(uid.to_string());
            }
        } else {
            self.accounts.push(Account {
                user_id: user_id.map(|s| s.to_string()),
                email: email.to_string(),
                tokens,
            });
        }
    }

    pub fn from_file(network_dir: &NetworkDirectory) -> Result<Self, ConnectCacheError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(CONNECT_CACHE_FILENAME);

        std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ConnectCacheError::NotFound,
                _ => ConnectCacheError::ReadingFile(format!("Reading settings file: {e}")),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<ConnectCache>(&file_content).map_err(|e| {
                    ConnectCacheError::ReadingFile(format!("Parsing settings file: {e}"))
                })
            })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    #[serde(default)]
    pub user_id: Option<String>,
    pub email: String,
    pub tokens: AccessTokenResponse,
}

impl Account {
    /// Primary lookup, by stable Liana-Connect user identifier.
    pub fn from_cache_by_user_id(
        network_dir: &NetworkDirectory,
        user_id: &str,
    ) -> Result<Option<Self>, ConnectCacheError> {
        ConnectCache::from_file(network_dir).map(|cache| {
            cache
                .accounts
                .into_iter()
                .find(|c| c.user_id.as_deref() == Some(user_id))
        })
    }

    /// Lookup by email — used by the account-picker UI and as a migration
    /// fallback when `user_id` is not yet known locally. Safe because emails
    /// are unique per Liana-Connect account (enforced by the backend).
    pub fn from_cache_by_email(
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
    client: &AuthClient,
    refresh: bool,
    user_id: Option<&str>,
) -> Result<AccessTokenResponse, ConnectCacheError> {
    let email = &client.email;
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ConnectCacheError::WritingFile(format!("Creating directory: {e}")))?;
    }

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {e}")))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {e:?}")))?;

    let mut cache = if file_exists {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {e}")))?;

        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(cache) => cache,
            Err(e) => {
                tracing::warn!("Something wrong with Liana-Connect cache file: {:?}", e);
                tracing::warn!("Liana-Connect cache file is reset");
                ConnectCache::default()
            }
        }
    } else {
        ConnectCache::default()
    };

    let existing = cache.accounts.iter().position(|cred| cred.email == *email);

    let (tokens_to_return, write_needed) = match existing {
        Some(idx) if current_tokens.expires_at < cache.accounts[idx].tokens.expires_at => {
            // Another process already wrote fresher tokens. Use those, but
            // still stamp user_id in place if we just learned it and the row
            // is still legacy — otherwise the freshness shortcut would leave
            // an unstamped row and filter_connect_cache could drop it on a
            // sibling wallet's deletion.
            let needs_stamp = matches!(
                (user_id, cache.accounts[idx].user_id.as_deref()),
                (Some(_), None)
            );
            if let (true, Some(uid)) = (needs_stamp, user_id) {
                cache.accounts[idx].user_id = Some(uid.to_string());
            } else {
                tracing::debug!(
                    "Liana-Connect authentication tokens are up to date, nothing to do"
                );
            }
            (cache.accounts[idx].tokens.clone(), needs_stamp)
        }
        _ => {
            let tokens = if refresh {
                client
                    .refresh_token(&current_tokens.refresh_token)
                    .await
                    .map_err(ConnectCacheError::Updating)?
            } else {
                current_tokens.clone()
            };
            cache.upsert_credential(user_id, email, tokens.clone());
            (tokens, true)
        }
    };

    if write_needed {
        let content = serde_json::to_vec_pretty(&cache).map_err(|e| {
            ConnectCacheError::WritingFile(format!("Failed to serialize settings: {e}"))
        })?;

        file.seek(SeekFrom::Start(0)).await.map_err(|e| {
            ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {e}"))
        })?;

        file.write_all(&content).await.map_err(|e| {
            tracing::warn!("failed to write to file: {:?}", e);
            ConnectCacheError::WritingFile(e.to_string())
        })?;

        file.inner_mut()
            .set_len(content.len() as u64)
            .await
            .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {e}")))?;
    }

    Ok(tokens_to_return)
}

pub async fn filter_connect_cache(
    network_dir: &NetworkDirectory,
    user_ids: &HashSet<String>,
    legacy_emails: &HashSet<String>,
) -> Result<(), ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ConnectCacheError::WritingFile(format!("Creating directory: {e}")))?;
    }

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {e}")))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {e:?}")))?;

    let mut cache = if file_exists {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {e}")))?;

        match serde_json::from_slice::<ConnectCache>(&file_content) {
            Ok(cache) => cache,
            Err(e) => {
                tracing::warn!("Something wrong with Liana-Connect cache file: {:?}", e);
                tracing::warn!("Liana-Connect cache file is reset");
                ConnectCache::default()
            }
        }
    } else {
        ConnectCache::default()
    };

    cache.accounts.retain(|a| match &a.user_id {
        Some(uid) => user_ids.contains(uid),
        None => legacy_emails.contains(&a.email),
    });

    let content = serde_json::to_vec_pretty(&cache).map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to serialize settings: {e}"))
    })?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {e}"))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        ConnectCacheError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {e}")))?;

    Ok(())
}

/// Stamp the authoritative `user_id` and `email` reported by Liana-Connect onto
/// the cache row for this user. Locates the row by `lookup_user_id` (preferred)
/// and falls back to `lookup_email` (covers legacy rows that lack `user_id`).
/// Also consolidates duplicates: after an OTP re-auth with a changed email
/// `update_connect_cache` may insert a fresh email-keyed row alongside the
/// existing user_id row — we keep the freshest tokens on a single canonical
/// row and drop the rest. No-op if no row matches.
pub async fn stamp_account_identity(
    network_dir: &NetworkDirectory,
    lookup_user_id: Option<&str>,
    lookup_email: &str,
    new_user_id: &str,
    new_email: &str,
) -> Result<(), ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
    if !file_exists {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .truncate(false)
        .open(&path)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Opening file: {e}")))?
        .lock_write()
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Locking file: {e:?}")))?;

    let mut file_content = Vec::new();
    file.read_to_end(&mut file_content)
        .await
        .map_err(|e| ConnectCacheError::ReadingFile(format!("Reading file content: {e}")))?;

    let mut cache = match serde_json::from_slice::<ConnectCache>(&file_content) {
        Ok(cache) => cache,
        Err(e) => {
            tracing::warn!("Cannot parse Liana-Connect cache file: {:?}", e);
            return Ok(());
        }
    };

    if !stamp_in_memory(
        &mut cache,
        lookup_user_id,
        lookup_email,
        new_user_id,
        new_email,
    ) {
        return Ok(());
    }

    let content = serde_json::to_vec_pretty(&cache).map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to serialize settings: {e}"))
    })?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        ConnectCacheError::WritingFile(format!("Failed to seek to start of file: {e}"))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        ConnectCacheError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| ConnectCacheError::WritingFile(format!("Failed to truncate file: {e}")))?;

    Ok(())
}

/// In-memory stamp + dedup. Returns true if anything changed.
fn stamp_in_memory(
    cache: &mut ConnectCache,
    lookup_user_id: Option<&str>,
    lookup_email: &str,
    new_user_id: &str,
    new_email: &str,
) -> bool {
    // Locate the canonical row: prefer the stable user_id, fall back to the
    // previous email so legacy (unstamped) rows can be promoted in place.
    let canonical_pos = lookup_user_id
        .and_then(|uid| {
            cache
                .accounts
                .iter()
                .position(|a| a.user_id.as_deref() == Some(uid))
        })
        .or_else(|| cache.accounts.iter().position(|a| a.email == lookup_email));

    let Some(canonical_pos) = canonical_pos else {
        return false;
    };

    // Among all rows that could plausibly be the same user (matching the new
    // identity, the previous email, or — when given — the previous user_id),
    // keep the freshest tokens.
    let is_candidate = |a: &Account| {
        a.user_id.as_deref() == Some(new_user_id)
            || a.email == new_email
            || a.email == lookup_email
            || lookup_user_id.is_some_and(|uid| a.user_id.as_deref() == Some(uid))
    };
    let freshest_tokens = cache
        .accounts
        .iter()
        .filter(|a| is_candidate(a))
        .map(|a| a.tokens.clone())
        .max_by_key(|t| t.expires_at)
        .unwrap_or_else(|| cache.accounts[canonical_pos].tokens.clone());

    let current = &cache.accounts[canonical_pos];
    let needs_update = current.user_id.as_deref() != Some(new_user_id)
        || current.email != new_email
        || current.tokens.expires_at != freshest_tokens.expires_at
        || cache.accounts.iter().enumerate().any(|(i, a)| {
            i != canonical_pos
                && (a.user_id.as_deref() == Some(new_user_id) || a.email == new_email)
        });
    if !needs_update {
        return false;
    }

    cache.accounts[canonical_pos] = Account {
        user_id: Some(new_user_id.to_string()),
        email: new_email.to_string(),
        tokens: freshest_tokens,
    };

    // Drop any other rows that now collide on user_id or email with the
    // canonical row. Only dedupe by user_id or by the *new* email — never by
    // the previous email alone, which could belong to an unrelated user.
    let mut i = 0;
    cache.accounts.retain(|a| {
        let keep = i == canonical_pos
            || (a.user_id.as_deref() != Some(new_user_id) && a.email != new_email);
        i += 1;
        keep
    });

    true
}

#[derive(Debug, Clone)]
pub enum ConnectCacheError {
    NotFound,
    ReadingFile(String),
    WritingFile(String),
    Unexpected(String),
    Updating(AuthError),
}
impl std::fmt::Display for ConnectCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "ConnectCache file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {e}"),
            Self::WritingFile(e) => write!(f, "Error while writing file: {e}"),
            Self::Unexpected(e) => write!(f, "Unexpected error: {e}"),
            Self::Updating(e) => write!(f, "Error while updating cache file: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(expires_at: i64) -> AccessTokenResponse {
        AccessTokenResponse {
            access_token: format!("access-{expires_at}"),
            expires_at,
            refresh_token: format!("refresh-{expires_at}"),
        }
    }

    #[test]
    fn upsert_preserves_existing_user_id_when_caller_passes_none() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: Some("uid-1".to_string()),
                email: "a@x".to_string(),
                tokens: tok(100),
            }],
        };

        cache.upsert_credential(None, "a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("uid-1"));
        assert_eq!(cache.accounts[0].tokens.expires_at, 200);
    }

    #[test]
    fn upsert_stamps_user_id_on_legacy_row() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: None,
                email: "a@x".to_string(),
                tokens: tok(100),
            }],
        };

        cache.upsert_credential(Some("uid-1"), "a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("uid-1"));
        assert_eq!(cache.accounts[0].tokens.expires_at, 200);
    }

    #[test]
    fn upsert_inserts_with_user_id_when_provided() {
        let mut cache = ConnectCache { accounts: vec![] };

        cache.upsert_credential(Some("uid-1"), "a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("uid-1"));
        assert_eq!(cache.accounts[0].email, "a@x");
    }

    #[test]
    fn upsert_inserts_with_no_user_id_when_caller_lacks_it() {
        let mut cache = ConnectCache { accounts: vec![] };

        cache.upsert_credential(None, "a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        assert!(cache.accounts[0].user_id.is_none());
        assert_eq!(cache.accounts[0].email, "a@x");
    }

    // Bug 1: settings already has user_id but the cache row is still legacy
    // (user_id=None). The user_id lookup misses; we must fall back to the
    // previous email so the legacy row gets stamped.
    #[test]
    fn stamp_promotes_legacy_row_when_user_id_lookup_misses() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: None,
                email: "old@x".to_string(),
                tokens: tok(100),
            }],
        };

        let changed = stamp_in_memory(&mut cache, Some("uid-1"), "old@x", "uid-1", "new@x");

        assert!(changed);
        assert_eq!(cache.accounts.len(), 1);
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("uid-1"));
        assert_eq!(cache.accounts[0].email, "new@x");
    }

    // Bug 2: forced OTP re-auth after server-side email change.
    // `update_connect_cache` (called just before `stamp_account_identity`)
    // inserted a fresh row keyed by the new email while the previously
    // stamped row still carries the same user_id with stale tokens. After
    // stamping there must be exactly ONE row, with the freshest tokens.
    #[test]
    fn stamp_dedupes_after_otp_with_changed_email() {
        let mut cache = ConnectCache {
            accounts: vec![
                Account {
                    user_id: Some("uid-1".to_string()),
                    email: "old@x".to_string(),
                    tokens: tok(100),
                },
                Account {
                    // Just inserted by `update_connect_cache`.
                    user_id: None,
                    email: "new@x".to_string(),
                    tokens: tok(500),
                },
            ],
        };

        let changed = stamp_in_memory(&mut cache, None, "new@x", "uid-1", "new@x");

        assert!(changed);
        assert_eq!(cache.accounts.len(), 1);
        let row = &cache.accounts[0];
        assert_eq!(row.user_id.as_deref(), Some("uid-1"));
        assert_eq!(row.email, "new@x");
        assert_eq!(row.tokens.expires_at, 500);
    }

    #[test]
    fn stamp_idempotent_when_already_canonical() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: Some("uid-1".to_string()),
                email: "a@x".to_string(),
                tokens: tok(500),
            }],
        };

        let changed = stamp_in_memory(&mut cache, Some("uid-1"), "a@x", "uid-1", "a@x");

        assert!(!changed);
        assert_eq!(cache.accounts.len(), 1);
    }

    #[test]
    fn stamp_noop_when_nothing_matches() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: Some("other-uid".to_string()),
                email: "other@x".to_string(),
                tokens: tok(100),
            }],
        };

        let changed = stamp_in_memory(&mut cache, Some("uid-1"), "old@x", "uid-1", "new@x");

        assert!(!changed);
        assert_eq!(cache.accounts.len(), 1);
        // The unrelated row is untouched.
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("other-uid"));
    }
}
