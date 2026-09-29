use liana::miniscript::bitcoin::Network;
use std::collections::HashSet;

use crate::{
    app::settings::{self, LianaSettings, SettingsError, WalletSettings},
    dir::NetworkDirectory,
    services::connect::{
        client::{
            auth::{AccessTokenResponse, AuthClient, AuthError},
            cache::{self, Account, ConnectCacheError},
            get_service_config, BackendType,
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

/// Best effort: close on the server the sessions of accounts whose cached
/// credentials were just dropped. Failures are logged and never propagated.
async fn close_connect_sessions(
    network: Network,
    backend_type: BackendType,
    accounts: Vec<Account>,
) {
    if accounts.is_empty() {
        return;
    }
    let config = match get_service_config(network, backend_type).await {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(
                "Failed to fetch Liana-Connect service config, sessions left open: {e}"
            );
            return;
        }
    };
    for account in accounts {
        let client = AuthClient::new(
            config.auth_api_url.clone(),
            config.auth_api_public_key.clone(),
            account.email,
            backend_type.user_agent(),
        );
        match close_connect_session(&client, &account.tokens).await {
            Ok(()) => tracing::info!("Closed Liana-Connect session of {}", client.email),
            Err(e) => tracing::error!(
                "Failed to close Liana-Connect session of {}: {e}",
                client.email
            ),
        }
    }
}

/// Log out with the cached access token, refreshing it first when it is
/// expired locally or rejected by the server.
async fn close_connect_session(
    client: &AuthClient,
    tokens: &AccessTokenResponse,
) -> Result<(), AuthError> {
    if tokens.expires_at >= chrono::Utc::now().timestamp() {
        match client.logout(&tokens.access_token).await {
            Err(AuthError {
                http_status: Some(401),
                ..
            }) => {}
            res => return res,
        }
    }
    let fresh = client.refresh_token(&tokens.refresh_token).await?;
    client.logout(&fresh.access_token).await
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

    let dropped = cache::filter_connect_cache(network_dir, &remaining_user_ids, &legacy_emails)
        .await
        .map_err(DeleteError::ConnectCache)?;
    close_connect_sessions(network, backend_type, dropped).await;

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

    if delete_liana_connect {
        if let Some(auth) = &wallet.remote_backend_auth {
            let service_config = get_service_config(network, backend_type)
                .await
                .map_err(|e| DeleteError::Connect(e.to_string()))?;

            let client = AuthClient::new(
                service_config.auth_api_url,
                service_config.auth_api_public_key,
                auth.email.to_string(),
                backend_type.user_agent(),
            );
            if let BackendState::WalletExists(client, _, _, _) = connect_with_credentials(
                client,
                auth.clone(),
                service_config.backend_api_url,
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

    let dropped = cache::filter_connect_cache(network_dir, &remaining_user_ids, &legacy_emails)
        .await
        .map_err(DeleteError::ConnectCache)?;
    close_connect_sessions(network, backend_type, dropped).await;

    signer::delete_wallet_mnemonics(
        network_dir,
        &wallet_id.descriptor_checksum,
        wallet_id.timestamp,
    )
    .map_err(DeleteError::Io)?;

    Ok(())
}
