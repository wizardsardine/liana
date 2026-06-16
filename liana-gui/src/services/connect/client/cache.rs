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
    /// Upsert tokens for the row matching `email`. Preserves the existing
    /// `user_id` field — user_id stamping is handled separately by
    /// `stamp_account_identity` (called from the login backfill path), so
    /// this function does not need to know the user_id.
    fn upsert_credential(&mut self, email: &str, tokens: AccessTokenResponse) {
        if let Some(c) = self.accounts.iter_mut().find(|c| c.email == email) {
            c.tokens = tokens;
        } else {
            self.accounts.push(Account {
                user_id: None,
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

    if let Some(c) = cache.accounts.iter().find(|cred| cred.email == *email) {
        // Another process updated the tokens
        if current_tokens.expires_at < c.tokens.expires_at {
            tracing::debug!("Liana-Connect authentication tokens are up to date, nothing to do");
            return Ok(c.tokens.clone());
        }
    }

    let tokens = if refresh {
        client
            .refresh_token(&current_tokens.refresh_token)
            .await
            .map_err(ConnectCacheError::Updating)?
    } else {
        current_tokens.clone()
    };

    cache.upsert_credential(email, tokens.clone());

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

    Ok(tokens)
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
/// the cache row for this user, locating it by `lookup_user_id` (preferred) or
/// the previous `lookup_email` (legacy / pre-migration). No-op if no row
/// matches, since the cache write that normally precedes this call inserts the
/// row keyed by email.
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

    let row = cache.accounts.iter_mut().find(|a| match lookup_user_id {
        Some(uid) => a.user_id.as_deref() == Some(uid),
        None => a.email == lookup_email,
    });

    let Some(row) = row else {
        return Ok(());
    };

    if row.user_id.as_deref() == Some(new_user_id) && row.email == new_email {
        return Ok(());
    }
    row.user_id = Some(new_user_id.to_string());
    row.email = new_email.to_string();

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
    fn upsert_replaces_tokens_for_existing_email() {
        let mut cache = ConnectCache {
            accounts: vec![Account {
                user_id: Some("uid-1".to_string()),
                email: "a@x".to_string(),
                tokens: tok(100),
            }],
        };

        cache.upsert_credential("a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        // Existing user_id is preserved — upsert no longer touches it.
        assert_eq!(cache.accounts[0].user_id.as_deref(), Some("uid-1"));
        assert_eq!(cache.accounts[0].tokens.expires_at, 200);
    }

    #[test]
    fn upsert_inserts_with_no_user_id_for_new_email() {
        let mut cache = ConnectCache { accounts: vec![] };

        cache.upsert_credential("a@x", tok(200));

        assert_eq!(cache.accounts.len(), 1);
        // user_id starts unset; backfill stamps it later via stamp_account_identity.
        assert!(cache.accounts[0].user_id.is_none());
        assert_eq!(cache.accounts[0].email, "a@x");
    }
}
