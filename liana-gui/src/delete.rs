use liana::miniscript::bitcoin::Network;
use std::collections::HashSet;

use crate::{
    app::settings::{self, LianaSettings, SettingsError, WalletSettings},
    dir::NetworkDirectory,
    services::connect::{
        client::{
            auth::AuthClient,
            cache::{self, ConnectCacheError},
            close_sessions, get_service_config, BackendType, ServiceConfig,
        },
        login::{connect_with_credentials, BackendState},
    },
    signer,
};

pub enum DeleteError {
    Io(std::io::Error),
    Settings(SettingsError),
    ConnectCache(ConnectCacheError),
    Connect(String),
}

impl std::fmt::Display for DeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Settings(e) => write!(f, "{e}"),
            Self::ConnectCache(e) => write!(f, "{e}"),
            Self::Connect(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for DeleteError {
    fn from(value: std::io::Error) -> Self {
        DeleteError::Io(value)
    }
}

fn ignore_not_found<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Prune from the connect cache every account no longer referenced by the
/// remaining wallets, closing their server-side session first so a failure
/// never leaves a reachable session without the credentials to close it.
///
/// `service_config` is the one the caller already fetched, if any; it is only
/// requested when some session actually needs closing.
async fn drop_unused_connect_accounts(
    network: Network,
    network_dir: &NetworkDirectory,
    backend_type: BackendType,
    service_config: Option<ServiceConfig>,
    user_ids: &HashSet<String>,
    legacy_emails: &HashSet<String>,
) -> Result<(), DeleteError> {
    let to_drop = cache::connect_cache_dropped_accounts(network_dir, user_ids, legacy_emails)
        .map_err(DeleteError::ConnectCache)?;

    if !to_drop.is_empty() {
        let config = match service_config {
            Some(config) => Some(config),
            None => match get_service_config(network, backend_type).await {
                Ok(config) => Some(config),
                Err(e) => {
                    tracing::error!(
                        "Failed to fetch Liana-Connect service config, sessions left open: {e}"
                    );
                    None
                }
            },
        };
        if let Some(config) = config {
            close_sessions(&config, backend_type, to_drop).await;
        }
    }

    cache::filter_connect_cache(network_dir, user_ids, legacy_emails)
        .await
        .map_err(DeleteError::ConnectCache)
}

pub async fn delete_failed_install(
    network: Network,
    network_dir: &NetworkDirectory,
    wallet_id: &settings::WalletId,
    backend_type: BackendType,
) -> Result<(), DeleteError> {
    let lianad_directory = network_dir.lianad_data_directory(wallet_id);

    if !wallet_id.is_legacy() {
        ignore_not_found(tokio::fs::remove_dir_all(lianad_directory.path()).await)?;
    } else {
        // if this is a legacy wallet, then it is the only wallet in the network directory.
        ignore_not_found(tokio::fs::remove_file(lianad_directory.sqlite_db_file_path()).await)?;
        ignore_not_found(
            tokio::fs::remove_dir_all(lianad_directory.lianad_watchonly_wallet_path()).await,
        )?;
        ignore_not_found(
            tokio::fs::remove_file(lianad_directory.path().join("daemon.toml")).await,
        )?;
    }

    let mut remaining_user_ids = HashSet::<String>::new();
    let mut legacy_emails = HashSet::<String>::new();
    settings::update_settings_file(network_dir, |mut settings: LianaSettings| {
        settings
            .wallets
            .retain(|settings| settings.wallet_id() != *wallet_id);
        for w in settings.wallets.iter() {
            if let Some(auth) = w.remote_backend_auth.as_ref() {
                match &auth.user_id {
                    Some(uid) => {
                        remaining_user_ids.insert(uid.clone());
                    }
                    None => {
                        legacy_emails.insert(auth.email.clone());
                    }
                }
            }
        }
        settings
    })
    .await
    .map_err(DeleteError::Settings)?;

    drop_unused_connect_accounts(
        network,
        network_dir,
        backend_type,
        None,
        &remaining_user_ids,
        &legacy_emails,
    )
    .await?;

    signer::delete_wallet_mnemonics(
        network_dir,
        &wallet_id.descriptor_checksum,
        wallet_id.timestamp,
    )
    .map_err(DeleteError::Io)?;

    Ok(())
}

pub async fn delete_wallet(
    network: Network,
    network_dir: &NetworkDirectory,
    wallet: &WalletSettings,
    delete_liana_connect: bool,
    backend_type: BackendType,
) -> Result<(), DeleteError> {
    let wallet_id = wallet.wallet_id();
    let lianad_directory = network_dir.lianad_data_directory(&wallet_id);

    if !wallet_id.is_legacy() {
        ignore_not_found(tokio::fs::remove_dir_all(lianad_directory.path()).await)?;
    } else {
        // if this is a legacy wallet, then it is the only wallet in the network directory.
        ignore_not_found(tokio::fs::remove_file(lianad_directory.sqlite_db_file_path()).await)?;
        ignore_not_found(
            tokio::fs::remove_dir_all(lianad_directory.lianad_watchonly_wallet_path()).await,
        )?;
        ignore_not_found(
            tokio::fs::remove_file(lianad_directory.path().join("daemon.toml")).await,
        )?;
    }

    let mut service_config = None;
    if delete_liana_connect {
        if let Some(auth) = &wallet.remote_backend_auth {
            let config = get_service_config(network, backend_type)
                .await
                .map_err(|e| DeleteError::Connect(e.to_string()))?;

            let client = AuthClient::new(
                config.auth_api_url.clone(),
                config.auth_api_public_key.clone(),
                auth.email.to_string(),
                backend_type.user_agent(),
            );
            if let BackendState::WalletExists(client, _, _, _) = connect_with_credentials(
                client,
                auth.clone(),
                config.backend_api_url.clone(),
                network,
                network_dir,
            )
            .await
            .map_err(|e| DeleteError::Connect(e.to_string()))?
            {
                tracing::info!("Deleting wallet on Liana-Connect {} backend", network);
                client
                    .delete_wallet()
                    .await
                    .map_err(|e| DeleteError::Connect(e.to_string()))?;
            } else {
                tracing::warn!("Wallet not found on the platform");
            }
            service_config = Some(config);
        }
    }

    let mut remaining_user_ids = HashSet::<String>::new();
    let mut legacy_emails = HashSet::<String>::new();
    settings::update_settings_file(network_dir, |mut settings: LianaSettings| {
        settings
            .wallets
            .retain(|settings| settings.wallet_id() != wallet_id);
        for w in settings.wallets.iter() {
            if let Some(auth) = w.remote_backend_auth.as_ref() {
                match &auth.user_id {
                    Some(uid) => {
                        remaining_user_ids.insert(uid.clone());
                    }
                    None => {
                        legacy_emails.insert(auth.email.clone());
                    }
                }
            }
        }
        settings
    })
    .await
    .map_err(DeleteError::Settings)?;

    drop_unused_connect_accounts(
        network,
        network_dir,
        backend_type,
        service_config,
        &remaining_user_ids,
        &legacy_emails,
    )
    .await?;

    signer::delete_wallet_mnemonics(
        network_dir,
        &wallet_id.descriptor_checksum,
        wallet_id.timestamp,
    )
    .map_err(DeleteError::Io)?;

    Ok(())
}
