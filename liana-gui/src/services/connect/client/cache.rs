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
    fn upsert_credential(&mut self, email: &str, tokens: AccessTokenResponse) {
        if let Some(c) = self.accounts.iter_mut().find(|c| c.email == email) {
            c.tokens = tokens;
        } else {
            self.accounts.push(Account {
                email: email.to_string(),
                tokens,
            })
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,
    pub tokens: AccessTokenResponse,
}

impl Account {
    pub fn from_cache(
        network_dir: &NetworkDirectory,
        email: &str,
    ) -> Result<Option<Self>, ConnectCacheError> {
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

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
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
                tracing::warn!("Something wrong with Liana-Connect cache file: {:?}", e);
                tracing::warn!("Liana-Connect cache file is reset");
                ConnectCache::default()
            }
        }
    } else {
        ConnectCache::default()
    };

    if let Some(c) = cache.accounts.iter().find(|cred| cred.email == *email) {
        // An other process updated the tokens
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

pub async fn filter_connect_cache(
    network_dir: &NetworkDirectory,
    emails: &HashSet<String>,
) -> Result<(), ConnectCacheError> {
    let mut path = network_dir.path().to_path_buf();
    path.push(CONNECT_CACHE_FILENAME);

    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
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
                tracing::warn!("Something wrong with Liana-Connect cache file: {:?}", e);
                tracing::warn!("Liana-Connect cache file is reset");
                ConnectCache::default()
            }
        }
    } else {
        ConnectCache::default()
    };

    cache.accounts.retain(|a| emails.contains(&a.email));

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
    Updating(AuthError),
}
impl std::fmt::Display for ConnectCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "ConnectCache file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
            Self::Updating(e) => write!(f, "Error while updating cache file: {}", e),
        }
    }
}
