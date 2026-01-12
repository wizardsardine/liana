pub mod auth;
pub mod backend;
pub mod cache;

use liana::miniscript::bitcoin;

use serde::Deserialize;

const DEFAULT_CONNECT_SIGNET_URL: &str = "https://api.connect.signet.lianawallet.com";
const DEFAULT_CONNECT_MAINNET_URL: &str = "https://api.connect.lianawallet.com";

/// Get Liana Lite API URL for the given network.
/// Environment variables can override the defaults for local testing:
/// - LIANA_CONNECT_SIGNET_API_URL: overrides only for signet/testnet
fn get_api_url(network: bitcoin::Network) -> String {
    if network == bitcoin::Network::Bitcoin {
        DEFAULT_CONNECT_MAINNET_URL.to_string()
    } else {
        std::env::var("LIANA_CONNECT_SIGNET_API_URL")
            .unwrap_or_else(|_| DEFAULT_CONNECT_SIGNET_URL.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ServiceConfigResource {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
}

pub async fn get_service_config(
    network: bitcoin::Network,
) -> Result<ServiceConfig, reqwest::Error> {
    let backend_api_url = get_api_url(network);
    let client = reqwest::Client::new();
    let res: ServiceConfigResource = client
        .get(format!("{}/v1/desktop", backend_api_url))
        .header("User-Agent", format!("liana-gui/{}", crate::VERSION))
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
