use liana::miniscript::bitcoin::Network;
use std::collections::HashSet;

use crate::{
    app::settings::{self, SettingsError, WalletSettings},
    dir::NetworkDirectory,
    services::connect::{
        client::{
            auth::AuthClient,
            cache::{self, ConnectCacheError},
            get_service_config,
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
            Self::Io(e) => write!(f, "{}", e),
            Self::Settings(e) => write!(f, "{}", e),
            Self::ConnectCache(e) => write!(f, "{}", e),
            Self::Connect(e) => write!(f, "{}", e),
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

pub async fn delete_failed_install(
    network_dir: &NetworkDirectory,
    wallet_id: &settings::WalletId,
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

    let mut remaining_accounts = HashSet::<String>::new();
    settings::update_settings_file(network_dir, |mut settings| {
        settings
            .wallets
            .retain(|settings| settings.wallet_id() != *wallet_id);
        remaining_accounts = settings
            .wallets
            .iter()
            .filter_map(|settings| {
                settings
                    .remote_backend_auth
                    .as_ref()
                    .map(|auth| auth.email.clone())
            })
            .collect();
        settings
    })
    .await
    .map_err(DeleteError::Settings)?;

    cache::filter_connect_cache(network_dir, &remaining_accounts)
        .await
        .map_err(DeleteError::ConnectCache)?;

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
            let service_config = get_service_config(network)
                .await
                .map_err(|e| DeleteError::Connect(e.to_string()))?;

            let client = AuthClient::new(
                service_config.auth_api_url,
                service_config.auth_api_public_key,
                auth.email.to_string(),
            );
            if let BackendState::WalletExists(client, _, _) = connect_with_credentials(
                client,
                auth.wallet_id.clone(),
                service_config.backend_api_url,
                network,
                network_dir,
            )
            .await
            .map_err(|e| DeleteError::Connect(e.to_string()))?
            {
                tracing::info!("Deleting  wallet on Liana-Connect {} plateform", network);
                client
                    .delete_wallet()
                    .await
                    .map_err(|e| DeleteError::Connect(e.to_string()))?;
            } else {
                tracing::warn!("Wallet not found on the platform");
            }
        }
    }

    let mut remaining_accounts = HashSet::<String>::new();
    settings::update_settings_file(network_dir, |mut settings| {
        settings
            .wallets
            .retain(|settings| settings.wallet_id() != wallet_id);
        remaining_accounts = settings
            .wallets
            .iter()
            .filter_map(|settings| {
                settings
                    .remote_backend_auth
                    .as_ref()
                    .map(|auth| auth.email.clone())
            })
            .collect();
        settings
    })
    .await
    .map_err(DeleteError::Settings)?;

    cache::filter_connect_cache(network_dir, &remaining_accounts)
        .await
        .map_err(DeleteError::ConnectCache)?;

    signer::delete_wallet_mnemonics(
        network_dir,
        &wallet_id.descriptor_checksum,
        wallet_id.timestamp,
    )
    .map_err(DeleteError::Io)?;

    Ok(())
}
