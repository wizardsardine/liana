//! First-launch bootstrap: register this desktop as a `SignerDevice` on
//! the Connect API so it can receive realtime session events and be
//! addressed as a signer target.
//!
//! Idempotent — if `cache.account.device_id` is already set, the bootstrap
//! is a no-op. Failures are non-fatal (best-effort): the caller logs the
//! error and continues; the next launch retries.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::dir::NetworkDirectory;
use crate::services::connect::client::auth::AccessTokenResponse;
use crate::services::connect::client::cache::{self, Account, ConnectCacheError};

use super::{
    create_channel, device::GrpcDeviceClient, interceptor::AuthInterceptor, CreateChannelError,
};

#[derive(Debug)]
pub enum BootstrapError {
    Cache(ConnectCacheError),
    Channel(CreateChannelError),
    Rpc(tonic::Status),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cache(e) => write!(f, "Connect cache error: {}", e),
            Self::Channel(e) => write!(f, "gRPC channel error: {}", e),
            Self::Rpc(s) => write!(f, "RegisterDevice RPC error: {}", s),
        }
    }
}

impl std::error::Error for BootstrapError {}

impl From<ConnectCacheError> for BootstrapError {
    fn from(e: ConnectCacheError) -> Self {
        Self::Cache(e)
    }
}

impl From<CreateChannelError> for BootstrapError {
    fn from(e: CreateChannelError) -> Self {
        Self::Channel(e)
    }
}

impl From<tonic::Status> for BootstrapError {
    fn from(s: tonic::Status) -> Self {
        Self::Rpc(s)
    }
}

/// Ensure the current account has a registered `SignerDevice` on the API.
///
/// If `cache.account.device_id` is `Some`, short-circuits and returns the
/// existing id without making any RPC. Otherwise calls `RegisterDevice`
/// and persists the returned id to the cache.
///
/// The `app_version` should be the desktop app version (e.g. from
/// `env!("CARGO_PKG_VERSION")`) and `os_version` should describe the host
/// OS (e.g. `std::env::consts::OS`). `device_name` is the user-visible
/// label the API stores — defaults to a hostname-style string if empty.
pub async fn ensure_device_registered(
    grpc_url: &str,
    tokens: Arc<RwLock<AccessTokenResponse>>,
    network_dir: &NetworkDirectory,
    email: &str,
    device_name: String,
    app_version: String,
    os_version: String,
) -> Result<String, BootstrapError> {
    if let Some(existing) = Account::from_cache(network_dir, email)
        .ok()
        .flatten()
        .and_then(|a| a.device_id)
    {
        tracing::debug!(
            "SignerDevice already registered for {} (device_id={})",
            email,
            existing,
        );
        return Ok(existing);
    }

    let channel = create_channel(grpc_url).await?;
    let access_token = tokens.read().await.access_token.clone();
    let mut device = GrpcDeviceClient::new(channel, AuthInterceptor::new(&access_token));

    let resp = device
        .register_device(device_name, app_version, os_version)
        .await?;

    cache::set_device_id_for_email(network_dir, email, Some(&resp.device_id)).await?;
    tracing::info!(
        "Registered SignerDevice for {} (device_id={})",
        email,
        resp.device_id,
    );
    Ok(resp.device_id)
}

/// Best-effort wrapper around `ensure_device_registered` that swallows
/// errors after logging. Use this from post-login orchestration points
/// where a registration failure shouldn't block the user from opening
/// the app.
pub async fn ensure_device_registered_best_effort(
    grpc_url: &str,
    tokens: Arc<RwLock<AccessTokenResponse>>,
    network_dir: &NetworkDirectory,
    email: &str,
    device_name: String,
    app_version: String,
    os_version: String,
) -> Option<String> {
    match ensure_device_registered(
        grpc_url,
        tokens,
        network_dir,
        email,
        device_name,
        app_version,
        os_version,
    )
    .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("Best-effort SignerDevice registration failed: {}", e);
            None
        }
    }
}
