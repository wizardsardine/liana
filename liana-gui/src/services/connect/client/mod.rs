pub mod auth;
pub mod backend;
pub mod cache;

use liana::miniscript::bitcoin::{self, Network};

use serde::Deserialize;

use auth::{AccessTokenResponse, AuthClient, AuthError};
use cache::Account;

const DEFAULT_CONNECT_SIGNET_URL: &str = "https://api.connect.signet.lianawallet.com";
const DEFAULT_CONNECT_MAINNET_URL: &str = "https://api.connect.lianawallet.com";

pub const BUSINESS_MAINNET_API_URL: &str = "https://api.connect.lianawallet.com";
pub const BUSINESS_SIGNET_API_URL: &str = "https://api.connect.signet.lianawallet.com";

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfigResource {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
}

#[derive(Debug, Clone, Copy)]
pub enum BackendType {
    LianaConnect,
    LianaBusiness(&'static str),
}

impl BackendType {
    pub fn user_agent(&self) -> String {
        match self {
            BackendType::LianaConnect => format!("liana-gui/{}", crate::VERSION),
            BackendType::LianaBusiness(version) => format!("liana-business/{version}"),
        }
    }
}

pub async fn get_service_config(
    network: bitcoin::Network,
    backend: BackendType,
) -> Result<ServiceConfig, reqwest::Error> {
    let backend_api_url = match (network, backend) {
        (Network::Bitcoin, BackendType::LianaConnect) => DEFAULT_CONNECT_MAINNET_URL.to_string(),
        (Network::Bitcoin, BackendType::LianaBusiness(_)) => BUSINESS_MAINNET_API_URL.to_string(),
        (_, BackendType::LianaConnect) => std::env::var("LIANALITE_SIGNET_API_URL")
            .unwrap_or_else(|_| DEFAULT_CONNECT_SIGNET_URL.to_string()),
        (_, BackendType::LianaBusiness(_)) => std::env::var("LIANA_BUSINESS_SIGNET_API_URL")
            .unwrap_or_else(|_| BUSINESS_SIGNET_API_URL.to_string()),
    };
    let client = reqwest::Client::new();
    let res: ServiceConfigResource = client
        .get(format!("{backend_api_url}/v1/desktop"))
        .header("User-Agent", backend.user_agent())
        .send()
        .await?
        .json()
        .await?;
    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url,
    })
}

/// Best effort: close on the server the sessions backing `accounts`, whose
/// cached credentials are about to be dropped. Failures are logged and never
/// propagated.
pub async fn close_sessions(
    service_config: &ServiceConfig,
    backend_type: BackendType,
    accounts: Vec<Account>,
) {
    for account in accounts {
        let client = AuthClient::new(
            service_config.auth_api_url.clone(),
            service_config.auth_api_public_key.clone(),
            account.email,
            backend_type.user_agent(),
        );
        match close_session(&client, &account.tokens).await {
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
async fn close_session(client: &AuthClient, tokens: &AccessTokenResponse) -> Result<(), AuthError> {
    if !tokens.is_expired() {
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
